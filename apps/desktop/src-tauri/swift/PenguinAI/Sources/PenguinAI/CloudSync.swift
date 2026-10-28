// Penguin ⇄ CloudKit: cross-device state (docs/CROSS-DEVICE-STATE.md).
//
// One CKSyncEngine on the user's private database, one custom zone
// ("Penguin"), one record type ("Record") whose only field is the
// encrypted `payload`: a record of the Rust core (crates/penguin-sync), as
// JSON. This file moves those records and reports what CloudKit says. It
// never reads a payload; merging, applying, choosing what to send and
// starting over all happen in the Rust core (penguin-core `cloud.rs`,
// `CloudHost`), reached through `CloudSyncHost`.
//
// The same file is compiled into both apps:
// - the Mac: this Swift package (built by src-tauri/build.rs), with the C
//   entry points in CloudSyncBridge.swift; `CloudSyncHost` is C callbacks
//   into Rust (src-tauri/src/cloud.rs).
// - the iPhone: ios/project.yml lists this file; `CloudSyncHost` is the
//   UniFFI core (ios/Penguin/Sync/).
//
// Nothing here logs record contents, and nothing leaves the device except
// through CloudKit.
import Foundation
#if canImport(CloudKit)
import CloudKit
#endif

/// A record as the server stores it (penguin-core `WireRecord`): its name,
/// the payload, CloudKit's archived system fields (base64; the change tag
/// the next save is conditional on) and when the server stored it.
struct CloudWireRecord: Codable {
    var name: String
    var payload: String
    var tag: String
    var modifiedMs: Int64
}

/// A record to send (penguin-core `WireOutgoing`).
struct CloudWireOutgoing: Codable {
    var name: String
    var payload: String
    var baseTag: String?
}

/// The Rust core, as the engine sees it (penguin-core `CloudHost`). Every
/// call is synchronous and may touch the database: never call it from the
/// main thread of an app that is drawing.
protocol CloudSyncHost: AnyObject {
    /// CKSyncEngine's saved state (`State.Serialization`, JSON).
    func engineState() -> Data?
    func saveEngineState(_ data: Data)
    /// Fetched changes: merge them, forget what the server deleted.
    func apply(records: [CloudWireRecord], deleted: [String])
    /// A fetch that started at `startedMs` finished.
    func fetched(startedMs: Int64)
    /// Names of the records waiting to be sent.
    func pendingNames(limit: Int) -> [String]
    /// The records among `names` still waiting to be sent.
    func outgoing(names: [String]) -> [CloudWireOutgoing]
    /// Records the server stored.
    func saved(records: [CloudWireRecord])
    /// A save lost to the server's record; returns whether ours won and
    /// must be sent again.
    func conflict(server: CloudWireRecord) -> Bool
    /// The server no longer has these records: send them as new.
    func clearBase(names: [String])
    /// The server's copy is gone: "signedOut", "switchedAccounts",
    /// "zoneDeleted", "zoneReset" or "purged" (penguin-core `ResetReason`).
    func reset(reason: String)
    /// The iCloud account: "available", "noAccount", "restricted",
    /// "temporarilyUnavailable" or "unknown". Returns whether records move.
    @discardableResult
    func reportAccount(_ account: String) -> Bool
    /// Something to show in Settings (penguin-sync `CloudProblem`); nil
    /// kind clears it.
    func reportProblem(kind: String?, code: Int, detail: String)
    /// Sync with iCloud is on and possible here.
    func shouldRun() -> Bool
    /// Something Settings shows changed.
    func statusChanged()
}

#if canImport(CloudKit)

/// Milliseconds since 1970.
func cloudNowMs() -> Int64 {
    Int64(Date().timeIntervalSince1970 * 1000)
}

@available(macOS 14.0, iOS 17.0, *)
final class CloudSync: CKSyncEngineDelegate, @unchecked Sendable {
    static let zoneName = "Penguin"
    static let recordType = "Record"
    static let payloadKey = "payload"
    /// Names asked for per round when queueing pending records.
    static let pendingLimit = 5000

    private let host: CloudSyncHost
    private let container: CKContainer
    private let zoneID: CKRecordZone.ID
    private let lock = NSLock()
    private var engine: CKSyncEngine?
    private var fetchStartedMs: Int64 = 0
    private var accountObserver: NSObjectProtocol?

    init(host: CloudSyncHost, container: CKContainer) {
        self.host = host
        self.container = container
        self.zoneID = CKRecordZone.ID(zoneName: Self.zoneName, ownerName: CKCurrentUserDefaultName)
    }

    private var current: CKSyncEngine? {
        lock.lock()
        defer { lock.unlock() }
        return engine
    }

    /// Start the engine (once; a second call does nothing). It fetches
    /// and sends by itself from here on, and hears other devices' changes
    /// through CloudKit's silent pushes.
    func start() {
        lock.lock()
        if engine != nil {
            lock.unlock()
            return
        }
        let saved = host.engineState().flatMap {
            try? JSONDecoder().decode(CKSyncEngine.State.Serialization.self, from: $0)
        }
        var config = CKSyncEngine.Configuration(
            database: container.privateCloudDatabase,
            stateSerialization: saved,
            delegate: self
        )
        config.automaticallySync = true
        let started = CKSyncEngine(config)
        engine = started
        lock.unlock()
        if saved == nil {
            started.state.add(pendingDatabaseChanges: [.saveZone(CKRecordZone(zoneID: zoneID))])
        }
        queuePending()
        if accountObserver == nil {
            accountObserver = NotificationCenter.default.addObserver(
                forName: .CKAccountChanged, object: nil, queue: nil
            ) { [weak self] _ in
                Task { await self?.checkAccount() }
            }
        }
        Task { await checkAccount() }
    }

    /// Stop syncing (Sync with iCloud turned off, or iCloud data deleted).
    /// Records stay on the device and on iCloud.
    func stop() {
        lock.lock()
        let old = engine
        engine = nil
        lock.unlock()
        if let observer = accountObserver {
            NotificationCenter.default.removeObserver(observer)
            accountObserver = nil
        }
        if let old {
            Task { await old.cancelOperations() }
        }
        host.statusChanged()
    }

    var running: Bool { current != nil }

    /// Hand the engine every record waiting to be sent (after a local
    /// change; the engine schedules the send itself).
    func queuePending() {
        guard let engine = current else { return }
        let names = host.pendingNames(limit: Self.pendingLimit)
        guard !names.isEmpty else { return }
        let changes: [CKSyncEngine.PendingRecordZoneChange] = names.map {
            .saveRecord(CKRecord.ID(recordName: $0, zoneID: zoneID))
        }
        engine.state.add(pendingRecordZoneChanges: changes)
    }

    /// Fetch now (a snooze is due and its wake needs a fresh fetch, the
    /// app came to the foreground, a background refresh). Returns when the
    /// fetch is done, or failed.
    func fetchNow() async {
        guard let engine = current else { return }
        do {
            try await engine.fetchChanges()
        } catch {
            report(error)
        }
    }

    /// Send what's waiting now instead of when the engine gets to it.
    func sendNow() async {
        queuePending()
        guard let engine = current else { return }
        do {
            try await engine.sendChanges()
        } catch {
            report(error)
        }
    }

    func checkAccount() async {
        do {
            let status = try await container.accountStatus()
            let name: String
            switch status {
            case .available: name = "available"
            case .noAccount: name = "noAccount"
            case .restricted: name = "restricted"
            case .temporarilyUnavailable: name = "temporarilyUnavailable"
            default: name = "unknown"
            }
            host.reportAccount(name)
        } catch {
            host.reportAccount("unknown")
            report(error)
        }
        host.statusChanged()
    }

    // MARK: CKSyncEngineDelegate

    func handleEvent(_ event: CKSyncEngine.Event, syncEngine: CKSyncEngine) async {
        switch event {
        case .stateUpdate(let update):
            if let data = try? JSONEncoder().encode(update.stateSerialization) {
                host.saveEngineState(data)
            }

        case .accountChange(let change):
            switch change.changeType {
            case .signIn:
                break
            case .signOut:
                host.reset(reason: "signedOut")
            case .switchAccounts:
                host.reset(reason: "switchedAccounts")
            @unknown default:
                break
            }
            // Whatever the account is now, it starts from an empty zone
            // or the one already there: make sure the zone exists and
            // everything this device has is queued.
            syncEngine.state.add(pendingDatabaseChanges: [.saveZone(CKRecordZone(zoneID: zoneID))])
            queuePending()
            await checkAccount()

        case .fetchedDatabaseChanges(let changes):
            for deletion in changes.deletions where deletion.zoneID == zoneID {
                let reason: String
                switch deletion.reason {
                case .deleted: reason = "zoneDeleted"
                case .purged: reason = "purged"
                case .encryptedDataReset: reason = "zoneReset"
                @unknown default: reason = "zoneDeleted"
                }
                host.reset(reason: reason)
                if host.shouldRun() {
                    syncEngine.state.add(pendingDatabaseChanges: [.saveZone(CKRecordZone(zoneID: zoneID))])
                    queuePending()
                } else {
                    stop()
                    return
                }
            }

        case .fetchedRecordZoneChanges(let changes):
            let records = changes.modifications.compactMap { wire($0.record) }
            let deleted = changes.deletions
                .filter { $0.recordID.zoneID == zoneID }
                .map { $0.recordID.recordName }
            if !records.isEmpty || !deleted.isEmpty {
                host.apply(records: records, deleted: deleted)
                // A record of ours that beat the fetched one goes out again.
                queuePending()
            }

        case .sentRecordZoneChanges(let sent):
            handleSent(sent, syncEngine: syncEngine)

        case .sentDatabaseChanges(let sent):
            for failed in sent.failedZoneSaves {
                report(failed.error)
            }

        case .willFetchChanges:
            fetchStartedMs = cloudNowMs()

        case .didFetchChanges:
            host.fetched(startedMs: fetchStartedMs > 0 ? fetchStartedMs : cloudNowMs())
            host.statusChanged()

        case .didSendChanges:
            host.statusChanged()

        default:
            break
        }
    }

    private func handleSent(_ sent: CKSyncEngine.Event.SentRecordZoneChanges, syncEngine: CKSyncEngine) {
        let saved = sent.savedRecords.compactMap { wire($0) }
        if !saved.isEmpty {
            host.saved(records: saved)
        }
        var again: [CKSyncEngine.PendingRecordZoneChange] = []
        var zoneMissing = false
        var problem: Error?
        for failed in sent.failedRecordSaves {
            let id = failed.record.recordID
            switch failed.error.code {
            case .serverRecordChanged:
                // Another device wrote first: merge its version; send ours
                // again (on top of the server's) if it still wins.
                if let server = failed.error.serverRecord, let w = wire(server) {
                    if host.conflict(server: w) {
                        again.append(.saveRecord(id))
                    }
                } else {
                    again.append(.saveRecord(id))
                }
            case .zoneNotFound:
                zoneMissing = true
                host.clearBase(names: [id.recordName])
                again.append(.saveRecord(id))
            case .unknownItem:
                host.clearBase(names: [id.recordName])
                again.append(.saveRecord(id))
            case .batchRequestFailed:
                again.append(.saveRecord(id))
            case .networkFailure, .networkUnavailable, .zoneBusy, .serviceUnavailable,
                 .requestRateLimited, .notAuthenticated, .operationCancelled,
                 .accountTemporarilyUnavailable:
                // The engine retries these by itself.
                break
            default:
                // Kept queued in the core (it is still dirty there); it goes
                // out with the next change or launch.
                problem = failed.error
            }
        }
        if zoneMissing {
            syncEngine.state.add(pendingDatabaseChanges: [.saveZone(CKRecordZone(zoneID: zoneID))])
        }
        if !again.isEmpty {
            syncEngine.state.add(pendingRecordZoneChanges: again)
        }
        if let problem {
            report(problem)
        } else if !saved.isEmpty {
            host.reportProblem(kind: nil, code: 0, detail: "")
        }
        host.statusChanged()
    }

    func nextRecordZoneChangeBatch(
        _ context: CKSyncEngine.SendChangesContext,
        syncEngine: CKSyncEngine
    ) async -> CKSyncEngine.RecordZoneChangeBatch? {
        let scope = context.options.scope
        let changes = syncEngine.state.pendingRecordZoneChanges.filter { scope.contains($0) }
        if changes.isEmpty { return nil }
        var names: [String] = []
        for change in changes {
            if case .saveRecord(let id) = change, id.zoneID == zoneID {
                names.append(id.recordName)
            }
        }
        let outgoing = Dictionary(
            host.outgoing(names: names).map { ($0.name, $0) },
            uniquingKeysWith: { first, _ in first }
        )
        let zoneID = self.zoneID
        return await CKSyncEngine.RecordZoneChangeBatch(pendingChanges: changes) { id in
            guard id.zoneID == zoneID, let o = outgoing[id.recordName] else {
                // Already sent, or not ours: nothing to do for it.
                syncEngine.state.remove(pendingRecordZoneChanges: [.saveRecord(id)])
                return nil
            }
            return CloudSync.record(o, id: id)
        }
    }

    // MARK: Records

    /// The CKRecord to save: the server's last version (so the save is
    /// conditional on its change tag) or a new one, with the payload.
    static func record(_ o: CloudWireOutgoing, id: CKRecord.ID) -> CKRecord {
        var record: CKRecord?
        if let tag = o.baseTag, let data = Data(base64Encoded: tag), let base = unarchive(data),
           base.recordID == id {
            record = base
        }
        let r = record ?? CKRecord(recordType: recordType, recordID: id)
        r.encryptedValues[payloadKey] = Data(o.payload.utf8)
        return r
    }

    private func wire(_ r: CKRecord) -> CloudWireRecord? {
        guard r.recordID.zoneID == zoneID, r.recordType == Self.recordType else { return nil }
        let data: Data? = r.encryptedValues[Self.payloadKey]
        guard let data, let payload = String(data: data, encoding: .utf8) else { return nil }
        let modified = r.modificationDate ?? Date()
        return CloudWireRecord(
            name: r.recordID.recordName,
            payload: payload,
            tag: Self.archive(r).base64EncodedString(),
            modifiedMs: Int64(modified.timeIntervalSince1970 * 1000)
        )
    }

    /// The record's system fields (its change tag among them), archived.
    static func archive(_ r: CKRecord) -> Data {
        let coder = NSKeyedArchiver(requiringSecureCoding: true)
        r.encodeSystemFields(with: coder)
        coder.finishEncoding()
        return coder.encodedData
    }

    static func unarchive(_ data: Data) -> CKRecord? {
        guard let coder = try? NSKeyedUnarchiver(forReadingFrom: data) else { return nil }
        coder.requiresSecureCoding = true
        defer { coder.finishDecoding() }
        return CKRecord(coder: coder)
    }

    // MARK: Problems

    private func report(_ error: Error) {
        guard let ck = error as? CKError else {
            host.reportProblem(kind: "other", code: -1, detail: String(describing: type(of: error)))
            host.statusChanged()
            return
        }
        switch ck.code {
        case .quotaExceeded:
            host.reportProblem(kind: "quotaExceeded", code: ck.code.rawValue, detail: "")
        case .badContainer, .missingEntitlement, .permissionFailure, .badDatabase,
             .serverRejectedRequest, .invalidArguments:
            // The container or its schema isn't ready for this build (in
            // Production: the schema not deployed yet).
            host.reportProblem(kind: "notSetUp", code: ck.code.rawValue, detail: Self.codeName(ck.code))
        case .notAuthenticated:
            Task { await checkAccount() }
        case .networkFailure, .networkUnavailable, .serviceUnavailable, .requestRateLimited,
             .zoneBusy, .operationCancelled:
            break
        default:
            host.reportProblem(kind: "other", code: ck.code.rawValue, detail: Self.codeName(ck.code))
        }
        host.statusChanged()
    }

    static func codeName(_ code: CKError.Code) -> String {
        switch code {
        case .badContainer: return "badContainer"
        case .missingEntitlement: return "missingEntitlement"
        case .permissionFailure: return "permissionFailure"
        case .badDatabase: return "badDatabase"
        case .serverRejectedRequest: return "serverRejectedRequest"
        case .invalidArguments: return "invalidArguments"
        case .internalError: return "internalError"
        case .partialFailure: return "partialFailure"
        case .limitExceeded: return "limitExceeded"
        case .incompatibleVersion: return "incompatibleVersion"
        case .assetFileNotFound, .assetFileModified, .assetNotAvailable: return "asset"
        case .userDeletedZone: return "userDeletedZone"
        default: return "code \(code.rawValue)"
        }
    }
}

#endif
