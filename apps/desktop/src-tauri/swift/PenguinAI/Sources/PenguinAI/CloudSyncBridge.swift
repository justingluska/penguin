// The Mac's C entry points for cross-device state (CloudSync.swift), called
// from Rust (src-tauri/src/cloud.rs). The engine reaches back into Rust
// through one callback: `host(context, op, json, length, replyContext,
// reply)`; Rust handles the operation and calls `reply` with JSON before it
// returns (or not at all when there's nothing to say). Operation numbers
// and JSON shapes are mirrored in cloud.rs.
//
// Sync needs macOS 14 (CKSyncEngine) and a build signed with the iCloud
// entitlement and a provisioning profile (release.yml); every other build
// reports itself unavailable and never touches CloudKit, which would stop
// the app on a missing entitlement.
import Foundation
#if os(macOS) && canImport(CloudKit)
import CloudKit
import Security

/// `(replyContext, bytes, length)`: copy the bytes before returning.
public typealias PenguinCloudReply = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void
/// `(context, op, bytes, length, replyContext, reply)`.
public typealias PenguinCloudHostCallback = @convention(c) (
    UnsafeMutableRawPointer?, Int32, UnsafePointer<UInt8>?, Int, UnsafeMutableRawPointer?, PenguinCloudReply
) -> Void

/// Operations (cloud.rs `Op`).
enum CloudOp: Int32 {
    case engineState = 1
    case saveEngineState = 2
    case apply = 3
    case fetched = 4
    case pendingNames = 5
    case outgoing = 6
    case saved = 7
    case conflict = 8
    case clearBase = 9
    case reset = 10
    case reportAccount = 11
    case reportProblem = 12
    case shouldRun = 13
    case statusChanged = 14
}

/// CloudSyncHost over the C callback, JSON both ways.
final class MacCloudHost: CloudSyncHost, @unchecked Sendable {
    private let context: UnsafeMutableRawPointer?
    private let callback: PenguinCloudHostCallback

    init(context: UnsafeMutableRawPointer?, callback: PenguinCloudHostCallback) {
        self.context = context
        self.callback = callback
    }

    private func call(_ op: CloudOp, _ body: Data = Data("{}".utf8)) -> Data {
        var reply = Data()
        withUnsafeMutablePointer(to: &reply) { replyPtr in
            body.withUnsafeBytes { raw in
                callback(
                    context, op.rawValue,
                    raw.bindMemory(to: UInt8.self).baseAddress, raw.count,
                    UnsafeMutableRawPointer(replyPtr),
                    { replyContext, bytes, length in
                        guard let replyContext, let bytes, length > 0 else { return }
                        replyContext.assumingMemoryBound(to: Data.self).pointee = Data(bytes: bytes, count: length)
                    }
                )
            }
        }
        return reply
    }

    private func call<T: Encodable>(_ op: CloudOp, json value: T) -> Data {
        call(op, (try? JSONEncoder().encode(value)) ?? Data("{}".utf8))
    }

    private func decode<T: Decodable>(_ type: T.Type, _ data: Data) -> T? {
        try? JSONDecoder().decode(type, from: data)
    }

    private struct State: Codable { var state: String? }
    private struct Apply: Encodable { var records: [CloudWireRecord]; var deleted: [String] }
    private struct Fetched: Encodable { var startedMs: Int64 }
    private struct Limit: Encodable { var limit: Int }
    private struct Names: Codable { var names: [String] }
    private struct Records: Encodable { var records: [CloudWireRecord] }
    private struct Outgoing: Decodable { var records: [CloudWireOutgoing] }
    private struct Flag: Decodable { var value: Bool }
    private struct Reason: Encodable { var reason: String }
    private struct Account: Encodable { var account: String }
    private struct Problem: Encodable { var kind: String?; var code: Int; var detail: String }

    func engineState() -> Data? {
        decode(State.self, call(.engineState))?.state.flatMap { Data(base64Encoded: $0) }
    }

    func saveEngineState(_ data: Data) {
        _ = call(.saveEngineState, json: State(state: data.base64EncodedString()))
    }

    func apply(records: [CloudWireRecord], deleted: [String]) {
        _ = call(.apply, json: Apply(records: records, deleted: deleted))
    }

    func fetched(startedMs: Int64) {
        _ = call(.fetched, json: Fetched(startedMs: startedMs))
    }

    func pendingNames(limit: Int) -> [String] {
        decode(Names.self, call(.pendingNames, json: Limit(limit: limit)))?.names ?? []
    }

    func outgoing(names: [String]) -> [CloudWireOutgoing] {
        decode(Outgoing.self, call(.outgoing, json: Names(names: names)))?.records ?? []
    }

    func saved(records: [CloudWireRecord]) {
        _ = call(.saved, json: Records(records: records))
    }

    func conflict(server: CloudWireRecord) -> Bool {
        decode(Flag.self, call(.conflict, json: server))?.value ?? true
    }

    func clearBase(names: [String]) {
        _ = call(.clearBase, json: Names(names: names))
    }

    func reset(reason: String) {
        _ = call(.reset, json: Reason(reason: reason))
    }

    @discardableResult
    func reportAccount(_ account: String) -> Bool {
        decode(Flag.self, call(.reportAccount, json: Account(account: account)))?.value ?? false
    }

    func reportProblem(kind: String?, code: Int, detail: String) {
        _ = call(.reportProblem, json: Problem(kind: kind, code: code, detail: detail))
    }

    func shouldRun() -> Bool {
        decode(Flag.self, call(.shouldRun))?.value ?? false
    }

    func statusChanged() {
        _ = call(.statusChanged)
    }
}

/// The running engine (macOS 14+); `Any` so this compiles for older targets.
private let bridgeLock = NSLock()
private var bridgeEngine: AnyObject?
/// Calls into the engine run here, one at a time, off Rust's threads.
private let bridgeQueue = DispatchQueue(label: "co.penguin.cloud-bridge")

@available(macOS 14.0, *)
private func runningEngine() -> CloudSync? {
    bridgeLock.lock()
    defer { bridgeLock.unlock() }
    return bridgeEngine as? CloudSync
}

/// Whether this app was signed with an iCloud container.
private func hasCloudEntitlement() -> Bool {
    guard let task = SecTaskCreateFromSelf(kCFAllocatorDefault) else { return false }
    let value = SecTaskCopyValueForEntitlement(
        task, "com.apple.developer.icloud-container-identifiers" as CFString, nil
    )
    if let ids = value as? [String] { return !ids.isEmpty }
    return false
}

// MARK: - C entry points

/// 0 sync can run here, 1 macOS older than 14, 2 this build has no iCloud
/// container (unsigned, or a self-build without one).
@_cdecl("penguin_cloud_availability")
public func penguin_cloud_availability() -> Int32 {
    guard #available(macOS 14.0, *) else { return 1 }
    return hasCloudEntitlement() ? 0 : 2
}

/// Start the engine with `callback` as its way into Rust. `context` must
/// stay valid until `penguin_cloud_stop` returns. Does nothing when sync
/// can't run here or is already running.
@_cdecl("penguin_cloud_start")
public func penguin_cloud_start(_ context: UnsafeMutableRawPointer?, _ callback: PenguinCloudHostCallback) {
    guard #available(macOS 14.0, *), hasCloudEntitlement() else { return }
    bridgeQueue.async {
        bridgeLock.lock()
        if bridgeEngine != nil {
            bridgeLock.unlock()
            return
        }
        let host = MacCloudHost(context: context, callback: callback)
        let engine = CloudSync(host: host, container: CKContainer.default())
        bridgeEngine = engine
        bridgeLock.unlock()
        engine.start()
    }
}

/// Stop the engine. Records stay where they are.
@_cdecl("penguin_cloud_stop")
public func penguin_cloud_stop() {
    guard #available(macOS 14.0, *) else { return }
    bridgeQueue.sync {
        bridgeLock.lock()
        let engine = bridgeEngine as? CloudSync
        bridgeEngine = nil
        bridgeLock.unlock()
        engine?.stop()
    }
}

/// Records are waiting to be sent: hand them to the engine.
@_cdecl("penguin_cloud_pending")
public func penguin_cloud_pending() {
    guard #available(macOS 14.0, *) else { return }
    bridgeQueue.async { runningEngine()?.queuePending() }
}

/// Fetch now (a due snooze waits for a fresh fetch, the app came forward).
@_cdecl("penguin_cloud_fetch")
public func penguin_cloud_fetch() {
    guard #available(macOS 14.0, *) else { return }
    bridgeQueue.async {
        guard let engine = runningEngine() else { return }
        Task { await engine.fetchNow() }
    }
}

/// Send what's waiting now.
@_cdecl("penguin_cloud_send")
public func penguin_cloud_send() {
    guard #available(macOS 14.0, *) else { return }
    bridgeQueue.async {
        guard let engine = runningEngine() else { return }
        Task { await engine.sendNow() }
    }
}

/// Ask CloudKit about the iCloud account again.
@_cdecl("penguin_cloud_check_account")
public func penguin_cloud_check_account() {
    guard #available(macOS 14.0, *) else { return }
    bridgeQueue.async {
        guard let engine = runningEngine() else { return }
        Task { await engine.checkAccount() }
    }
}

#else

// Not a Mac (or no CloudKit in the SDK): sync is never available.
@_cdecl("penguin_cloud_availability")
public func penguin_cloud_availability() -> Int32 { 2 }

#endif
