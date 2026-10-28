// Handoff between the Mac and the iPhone (src-tauri/src/handoff.rs has the
// contract and the rules). Rust says what to advertise; this keeps one
// NSUserActivity current for it, and hands activities continued from the
// iPhone back to Rust.
//
// Publishing: `penguin_handoff_publish(type, title, userInfo JSON)` and
// `penguin_handoff_resign()`, from any thread; the work happens on the main
// thread, in call order. The same type again updates the current activity in
// place (title, userInfo); another type invalidates it and starts a new one.
//
// Receiving: tao's app delegate (TaoAppDelegateParent) implements
// `application:continueUserActivity:restorationHandler:` for web browsing
// activities only, and Tauri has no hook for others. `penguin_handoff_install`
// wraps that method on the delegate's class, once: Penguin's two types go
// to Rust's callback (the type and the userInfo as JSON) and are handled
// (YES); everything else calls tao's implementation as before.
// `application:willContinueUserActivityWithType:` stays tao's (NO for these
// types: macOS shows its own indication while the activity comes over).
import Foundation
#if os(macOS)
import AppKit
import ObjectiveC

/// `(type, typeLength, userInfoJSON, length)`: valid for the call only.
public typealias PenguinHandoffContinued = @convention(c) (UnsafePointer<UInt8>?, Int, UnsafePointer<UInt8>?, Int) -> Void

private let penguinActivityTypes: Set<String> = ["co.gluska.penguin.conversation", "co.gluska.penguin.draft"]

/// Main thread only.
private var handoffActivity: NSUserActivity?
private var handoffCallback: PenguinHandoffContinued?
private var handoffHookInstalled = false

private func copyString(_ bytes: UnsafePointer<UInt8>?, _ length: Int) -> String {
    guard let bytes, length > 0 else { return "" }
    return String(decoding: UnsafeBufferPointer(start: bytes, count: length), as: UTF8.self)
}

private func onMain(_ work: @escaping () -> Void) {
    if Thread.isMainThread {
        work()
    } else {
        DispatchQueue.main.async(execute: work)
    }
}

// MARK: - Publishing

private func publish(type: String, title: String, info: [String: Any]) {
    if let current = handoffActivity, current.activityType == type {
        current.title = title
        current.userInfo = info
        current.requiredUserInfoKeys = Set(info.keys)
        current.needsSave = true
        current.becomeCurrent()
        return
    }
    handoffActivity?.invalidate()
    let activity = NSUserActivity(activityType: type)
    activity.title = title
    activity.userInfo = info
    activity.requiredUserInfoKeys = Set(info.keys)
    activity.isEligibleForHandoff = true
    // Ids and a subject are not for Spotlight or Siri. (No prediction
    // property on macOS.)
    activity.isEligibleForSearch = false
    activity.isEligibleForPublicIndexing = false
    handoffActivity = activity
    activity.becomeCurrent()
}

/// Advertise `type` with `title` and `info` (a JSON object). Copies every
/// buffer before returning.
@_cdecl("penguin_handoff_publish")
public func penguin_handoff_publish(
    _ type: UnsafePointer<UInt8>?, _ typeLength: Int,
    _ title: UnsafePointer<UInt8>?, _ titleLength: Int,
    _ info: UnsafePointer<UInt8>?, _ infoLength: Int
) {
    let type = copyString(type, typeLength)
    let title = copyString(title, titleLength)
    let data = info.map { Data(bytes: $0, count: max(infoLength, 0)) } ?? Data()
    guard penguinActivityTypes.contains(type),
          let object = try? JSONSerialization.jsonObject(with: data),
          let userInfo = object as? [String: Any]
    else { return }
    onMain { publish(type: type, title: title, info: userInfo) }
}

/// Nothing to advertise: the activity stops being current (and is reused
/// when the same type comes back).
@_cdecl("penguin_handoff_resign")
public func penguin_handoff_resign() {
    onMain { handoffActivity?.resignCurrent() }
}

// MARK: - Receiving

/// The userInfo as a JSON object: string keys, JSON values only.
private func userInfoJSON(_ activity: NSUserActivity) -> Data {
    var info: [String: Any] = [:]
    for (key, value) in activity.userInfo ?? [:] {
        guard let key = key as? String else { continue }
        if JSONSerialization.isValidJSONObject([value]) { info[key] = value }
    }
    return (try? JSONSerialization.data(withJSONObject: info)) ?? Data("{}".utf8)
}

private func deliver(_ activity: NSUserActivity) {
    guard let callback = handoffCallback else { return }
    let type = Data(activity.activityType.utf8)
    let info = userInfoJSON(activity)
    type.withUnsafeBytes { t in
        info.withUnsafeBytes { i in
            callback(
                t.bindMemory(to: UInt8.self).baseAddress, t.count,
                i.bindMemory(to: UInt8.self).baseAddress, i.count
            )
        }
    }
}

/// `-[delegate application:continueUserActivity:restorationHandler:]`. The
/// restoration handler is a block, passed through as an object.
private typealias ContinueIMP = @convention(c) (AnyObject, Selector, AnyObject, NSUserActivity, AnyObject?) -> ObjCBool
private typealias ContinueBlock = @convention(block) (AnyObject, AnyObject, NSUserActivity, AnyObject?) -> ObjCBool

/// 0 installed (or already), 1 no app delegate yet, 2 no type encoding for the method.
private func installContinueHook() -> Int32 {
    if handoffHookInstalled { return 0 }
    guard let delegate = NSApplication.shared.delegate, let cls = object_getClass(delegate) else { return 1 }
    let sel = NSSelectorFromString("application:continueUserActivity:restorationHandler:")
    let existing = class_getInstanceMethod(cls, sel)
    let original: ContinueIMP? = existing.map { unsafeBitCast(method_getImplementation($0), to: ContinueIMP.self) }
    let types: UnsafePointer<CChar>? = existing.flatMap { method_getTypeEncoding($0) }
        ?? objc_getProtocol("NSApplicationDelegate").flatMap { proto in
            protocol_getMethodDescription(proto, sel, false, true).types.map { UnsafePointer($0) }
        }
    guard let types else { return 2 }
    let block: ContinueBlock = { this, app, activity, restoration in
        if penguinActivityTypes.contains(activity.activityType) {
            deliver(activity)
            return true
        }
        guard let original else { return false }
        return original(this, sel, app, activity, restoration)
    }
    // Replaces the method on the delegate's own class (adds it there when
    // only a superclass had it), so nothing else is touched.
    class_replaceMethod(cls, sel, imp_implementationWithBlock(block), types)
    handoffHookInstalled = true
    return 0
}

/// Start handing continued activities to `callback` (kept for the app's
/// lifetime). Call on the main thread once the app delegate is set (Tauri's
/// setup): answers installContinueHook's code, or 3 when called off the main
/// thread (installed there later, unchecked).
@_cdecl("penguin_handoff_install")
public func penguin_handoff_install(_ callback: PenguinHandoffContinued) -> Int32 {
    guard Thread.isMainThread else {
        DispatchQueue.main.async {
            handoffCallback = callback
            _ = installContinueHook()
        }
        return 3
    }
    handoffCallback = callback
    return installContinueHook()
}

#endif
