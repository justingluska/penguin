// Penguin ⇄ Apple Foundation Models: the on-device model behind thread
// summaries, the composer's writing (Write with AI, suggested replies) and
// reading Ask questions the grammar can't (`MailQuery`).
// Rust calls the `@_cdecl` functions below (src/summary/apple.rs);
// everything about *what* to ask (instructions, prompts, chunking, parsing)
// lives in Rust, so this file only runs one generation at a time and reports
// back. Design and sources: docs/SUMMARIES.md.
//
// Threading: `penguin_ai_generate` returns at once. The generation runs in a
// Swift Task and calls the Rust callback with snapshots, then exactly one
// terminal event (done or error), from whatever thread it's on. The context
// pointer belongs to Rust, which frees it on the terminal event; Swift never
// touches it after that.
//
// Nothing here logs, and no message text or model output leaves this file
// except through the callback.
//
// Compile-time guards (checked on CI with Xcode 26.0.1, 26.x and 27):
// - `canImport(FoundationModels)`: SDKs older than macOS 26 have no framework;
//   everything reports "not built".
// - `compiler(>=6.3)` ≈ Xcode 26.4+ (macOS 26.4 SDK): `contextSize` and
//   `tokenCount(for:)`.
// - `compiler(>=6.4)` ≈ Xcode 27 (macOS 27 SDK): the new error types
//   (`LanguageModelError` & co.). Apps built with Xcode 27 get those on
//   macOS 27; `GenerationError` is still what macOS 26 throws.
import Foundation
#if canImport(FoundationModels)
import FoundationModels
#endif

/// `(context, kind, bytes, length)`. kind 0 = snapshot, 1 = done, 2 = error;
/// bytes are UTF-8 JSON, valid only during the call.
public typealias PenguinAIEventCallback = @convention(c) (UnsafeMutableRawPointer?, Int32, UnsafePointer<UInt8>?, Int) -> Void

private let eventSnapshot: Int32 = 0
private let eventDone: Int32 = 1
private let eventError: Int32 = 2

// MARK: - C entry points

/// Bit 0: built with FoundationModels. Bit 1: with the macOS 26.4 token
/// APIs. Bit 2: with the Xcode 27 error types.
@_cdecl("penguin_ai_build_features")
public func penguin_ai_build_features() -> Int32 {
    var features: Int32 = 0
    #if canImport(FoundationModels)
    features |= 1
    #if compiler(>=6.3)
    features |= 2
    #endif
    #if compiler(>=6.4)
    features |= 4
    #endif
    #endif
    return features
}

/// 0 available, 1 device not eligible, 2 Apple Intelligence not enabled,
/// 3 model not ready, 4 macOS older than 26, 5 built without the SDK,
/// 6 a reason this build doesn't know.
@_cdecl("penguin_ai_availability")
public func penguin_ai_availability() -> Int32 {
    #if canImport(FoundationModels)
    guard #available(macOS 26.0, *) else { return 4 }
    return FM.availabilityCode()
    #else
    return 5
    #endif
}

/// The model's context window in tokens, or -1 when this build can't ask
/// (`contextSize` needs the macOS 26.4 SDK; it is back-deployed to 26.0).
@_cdecl("penguin_ai_context_size")
public func penguin_ai_context_size() -> Int64 {
    #if canImport(FoundationModels) && compiler(>=6.3)
    if #available(macOS 26.0, *) {
        return Int64(SystemLanguageModel.default.contextSize)
    }
    #endif
    return -1
}

/// Load the model and cache `instructions` ahead of a summary the user is
/// about to ask for (Apple: prewarm only with at least a second to spare).
/// The next guided generation with the same instructions uses that session.
@_cdecl("penguin_ai_prewarm")
public func penguin_ai_prewarm(_ instructions: UnsafePointer<UInt8>?, _ length: Int) {
    #if canImport(FoundationModels)
    guard #available(macOS 26.0, *), let instructions, length > 0 else { return }
    let text = String(decoding: UnsafeBufferPointer(start: instructions, count: length), as: UTF8.self)
    FM.prewarm(instructions: text)
    #endif
}

/// Start one generation described by the JSON `Request`. Returns a handle
/// for `penguin_ai_cancel`, or 0 when it failed at once (the error event has
/// then already been sent, before this returns).
@_cdecl("penguin_ai_generate")
public func penguin_ai_generate(
    _ request: UnsafePointer<UInt8>?,
    _ length: Int,
    _ context: UnsafeMutableRawPointer?,
    _ callback: PenguinAIEventCallback
) -> UInt64 {
    let sink = Sink(context: context, callback: callback)
    let data = request.map { Data(bytes: $0, count: length) } ?? Data()
    guard let req = try? JSONDecoder().decode(Request.self, from: data) else {
        sink.fail(code: "invalidRequest")
        return 0
    }
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        return Jobs.shared.start { id in
            Task(priority: .userInitiated) {
                await FM.run(req, sink)
                Jobs.shared.finished(id)
            }
        }
    }
    #endif
    sink.fail(code: "unavailable")
    return 0
}

/// Cancel a running generation. Its terminal event is an error with code
/// "cancelled" (or done, if it finished first).
@_cdecl("penguin_ai_cancel")
public func penguin_ai_cancel(_ handle: UInt64) {
    // Jobs holds Tasks: Swift concurrency, which macOS 11 doesn't have (it's
    // weak-linked), so nothing outside #available may touch it.
    if #available(macOS 26.0, *) {
        Jobs.shared.cancel(handle)
    }
}

// MARK: - Plumbing

struct Request: Decodable {
    var instructions: String
    var prompt: String
    /// Guided generation (`ThreadDigest`); false = plain text.
    var guided: Bool
    /// `.permissiveContentTransformations` guardrails (plain text only).
    var permissive: Bool
    /// Report snapshots as they stream.
    var stream: Bool
    var maxResponseTokens: Int
    var temperature: Double?
    /// Which `@Generable` type a guided generation fills: nil or "digest" =
    /// `ThreadDigest`, "replies" = `ReplyOptions`, "query" = `MailQuery`.
    /// Ignored for plain text.
    var schema: String?

    /// Guided into `ReplyOptions` (suggested replies).
    var isReplies: Bool { guided && schema == "replies" }
    /// Guided into `MailQuery` (an Ask question as a query).
    var isQuery: Bool { guided && schema == "query" }
}

/// Delivers events to Rust: any number of snapshots, then one terminal
/// event, after which nothing more is sent (the context may be freed).
final class Sink: @unchecked Sendable {
    private let context: UnsafeMutableRawPointer?
    private let callback: PenguinAIEventCallback
    private let lock = NSLock()
    private var ended = false

    init(context: UnsafeMutableRawPointer?, callback: PenguinAIEventCallback) {
        self.context = context
        self.callback = callback
    }

    func send(_ kind: Int32, _ data: Data) {
        lock.lock()
        defer { lock.unlock() }
        if ended { return }
        if kind != eventSnapshot { ended = true }
        data.withUnsafeBytes { raw in
            callback(context, kind, raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
        }
    }

    func send<T: Encodable>(_ kind: Int32, json value: T) {
        let data = (try? JSONEncoder().encode(value)) ?? Data("{}".utf8)
        send(kind, data)
    }

    func fail(code: String, tokens: Int? = nil, limit: Int? = nil, type: String? = nil) {
        send(eventError, json: WireError(code: code, tokens: tokens, limit: limit, type: type))
    }
}

struct WireError: Encodable {
    var code: String
    var tokens: Int?
    var limit: Int?
    /// The Swift error type, for "other" (never its description, which can
    /// quote generated text).
    var type: String?
}

struct WireItem: Encodable {
    var source: Int?
    var text: String?
    var due: String?
}

/// `MailQuery` as Rust's `ask::QueryDraft` reads it (same field names).
struct WireQuery: Encodable {
    var subject: String
    var op: String
    var measure: String
    var groupBy: String
    var timeframe: String
    var compare: [String]
    var place: String
    var merchant: String
    var person: String
    var direction: String
    var field: String
    var tense: String
}

/// `ThreadDigest` (partial or whole) as Rust's `summary::Draft` reads it;
/// `replies` for `ReplyOptions`; or `text` for a plain-text generation.
struct WireDigest: Encodable {
    var points: [WireItem]?
    var asks: [WireItem]?
    var gist: String?
    var text: String?
    var replies: [String]?
}

@available(macOS 26.0, *)
final class Jobs: @unchecked Sendable {
    static let shared = Jobs()
    private let lock = NSLock()
    private var next: UInt64 = 1
    private var tasks: [UInt64: Task<Void, Never>] = [:]

    func start(_ make: (UInt64) -> Task<Void, Never>) -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        let id = next
        next += 1
        // The task can't reach `finished` before it's recorded: that needs
        // this lock.
        tasks[id] = make(id)
        return id
    }

    func finished(_ id: UInt64) {
        lock.lock()
        tasks[id] = nil
        lock.unlock()
    }

    func cancel(_ id: UInt64) {
        lock.lock()
        let task = tasks[id]
        lock.unlock()
        task?.cancel()
    }
}

// MARK: - Foundation Models

#if canImport(FoundationModels)

/// What the model fills in. Apple generates properties in declaration order
/// and recommends putting the summary after the details it should draw on
/// (WWDC25 "Meet the Foundation Models framework" / "Deep dive"), so the
/// points and requests come first and the gist last. Each item names its
/// source message first, then describes it. The caps match Rust's
/// `summary::MAX_POINTS` / `MAX_ASKS`.
@available(macOS 26.0, *)
@Generable(description: "A summary of an email conversation for the person reading it")
struct ThreadDigest {
    @Guide(description: "The key facts, decisions and updates, oldest first", .maximumCount(6))
    var points: [DigestPoint]

    @Guide(description: "Things someone asked You to do, answer or decide; empty if none", .maximumCount(4))
    var asks: [DigestAsk]

    @Guide(description: "One or two sentences summarizing the whole conversation")
    var gist: String
}

@available(macOS 26.0, *)
@Generable
struct DigestPoint {
    @Guide(description: "The number of the message this comes from, like 3 for [#3]")
    var source: Int

    @Guide(description: "One short sentence")
    var text: String
}

@available(macOS 26.0, *)
@Generable
struct DigestAsk {
    @Guide(description: "The number of the message the request is in")
    var source: Int

    @Guide(description: "What You are asked to do, in one short sentence")
    var text: String

    @Guide(description: "The deadline exactly as the message states it, or an empty string if there is none")
    var due: String
}

/// Suggested replies to the newest message of a conversation (instant
/// replies in the composer). Rust (`penguin_core::writing`) tidies, dedupes
/// and caps them at `MAX_SUGGESTIONS`, which matches the count here.
@available(macOS 26.0, *)
@Generable(description: "Short replies the reader could send to the newest email")
struct ReplyOptions {
    @Guide(description: "Three different one-sentence replies of at most 12 words each, without greeting or sign-off", .count(3))
    var replies: [String]
}

/// A question about the person's own mail, as a query Penguin answers
/// exactly from local data (docs/ASK.md). The model only reads the question:
/// it never sees mail, and it never writes the answer. Every string is one
/// of a fixed set (constrained decoding, `anyOf`) or text Rust then checks
/// against its date grammar and the mailbox. Field names match Rust's
/// `ask::QueryDraft`; the order follows the question's parts, subject first.
@available(macOS 26.0, *)
@Generable(description: "A question about the user's own email, as a query")
struct MailQuery {
    @Guide(description: "What the question is about", .anyOf(["flights", "stays", "orders", "parcels", "bills", "bookings", "spending", "messages"]))
    var subject: String

    @Guide(description: "What to compute", .anyOf(["count", "sum", "average", "max", "min", "list", "first", "last", "next", "exists"]))
    var op: String

    @Guide(.anyOf(["items", "money", "nights", "trips"]))
    var measure: String

    @Guide(.anyOf(["none", "month", "year", "merchant", "place", "person"]))
    var groupBy: String

    @Guide(description: "The time words exactly as asked, or empty")
    var timeframe: String

    @Guide(description: "Two time periods or two names compared, else empty", .maximumCount(2))
    var compare: [String]

    @Guide(description: "A city or country, or empty")
    var place: String

    @Guide(description: "A store, airline, hotel or company, or empty")
    var merchant: String

    @Guide(description: "A person's name, or empty")
    var person: String

    @Guide(.anyOf(["any", "from", "to"]))
    var direction: String

    @Guide(description: "The one detail asked for", .anyOf(["none", "date", "confirmation", "flightNumber", "tracking", "orderNumber", "amount", "address"]))
    var field: String

    @Guide(.anyOf(["any", "past", "future"]))
    var tense: String
}

@available(macOS 26.0, *)
enum FM {
    static func availabilityCode() -> Int32 {
        switch SystemLanguageModel.default.availability {
        case .available:
            return 0
        case .unavailable(let reason):
            switch reason {
            case .deviceNotEligible: return 1
            case .appleIntelligenceNotEnabled: return 2
            case .modelNotReady: return 3
            @unknown default: return 6
            }
        @unknown default:
            return 6
        }
    }

    // MARK: Prewarm

    private final class Warm: @unchecked Sendable {
        let lock = NSLock()
        var instructions = ""
        var session: LanguageModelSession?
        var at = Date.distantPast
    }

    private static let warm = Warm()

    static func prewarm(instructions: String) {
        guard SystemLanguageModel.default.isAvailable else { return }
        warm.lock.lock()
        defer { warm.lock.unlock() }
        if warm.session != nil, warm.instructions == instructions,
           Date().timeIntervalSince(warm.at) < 120 {
            return
        }
        let session = LanguageModelSession(model: SystemLanguageModel.default, instructions: instructions)
        session.prewarm()
        warm.instructions = instructions
        warm.session = session
        warm.at = Date()
    }

    /// The prewarmed session, once, for a guided request with the same
    /// instructions made within five minutes of prewarming.
    private static func takeWarm(_ req: Request) -> LanguageModelSession? {
        guard req.guided, !req.permissive, !req.isReplies, !req.isQuery else { return nil }
        warm.lock.lock()
        defer { warm.lock.unlock() }
        guard let session = warm.session, warm.instructions == req.instructions,
              Date().timeIntervalSince(warm.at) < 300, !session.isResponding
        else { return nil }
        warm.session = nil
        return session
    }

    // MARK: Generation

    static func run(_ req: Request, _ sink: Sink) async {
        do {
            let model = req.permissive
                ? SystemLanguageModel(useCase: .general, guardrails: .permissiveContentTransformations)
                : SystemLanguageModel.default
            guard model.isAvailable else {
                sink.fail(code: "unavailable")
                return
            }
            if let (tokens, limit) = try await overBudget(model, req) {
                sink.fail(code: "contextExceeded", tokens: tokens, limit: limit)
                return
            }
            try Task.checkCancellation()
            let options = GenerationOptions(
                temperature: req.temperature,
                maximumResponseTokens: req.maxResponseTokens
            )
            // A new session per generation: each map step and the final
            // summary start from an empty transcript (TN3193).
            let session = takeWarm(req) ?? LanguageModelSession(model: model, instructions: req.instructions)
            if req.isReplies {
                try await replies(session, req, options, sink)
            } else if req.isQuery {
                try await query(session, req, options, sink)
            } else if req.guided {
                try await guided(session, req, options, sink)
            } else {
                try await text(session, req, options, sink)
            }
        } catch {
            let (code, tokens, limit) = describe(error)
            sink.fail(code: code, tokens: tokens, limit: limit, type: code == "other" ? String(describing: type(of: error)) : nil)
        }
    }

    private static func guided(_ session: LanguageModelSession, _ req: Request, _ options: GenerationOptions, _ sink: Sink) async throws {
        if !req.stream {
            let response = try await session.respond(to: req.prompt, generating: ThreadDigest.self, options: options)
            sink.send(eventDone, json: wire(response.content.asPartiallyGenerated()))
            return
        }
        // Snapshots, not deltas: each one is the whole digest so far, and
        // the last one is complete.
        var last: WireDigest?
        let stream = session.streamResponse(to: req.prompt, generating: ThreadDigest.self, options: options)
        for try await snapshot in stream {
            try Task.checkCancellation()
            let digest = wire(snapshot.content)
            last = digest
            sink.send(eventSnapshot, json: digest)
        }
        try Task.checkCancellation()
        sink.send(eventDone, json: last ?? WireDigest())
    }

    private static func replies(_ session: LanguageModelSession, _ req: Request, _ options: GenerationOptions, _ sink: Sink) async throws {
        if !req.stream {
            let response = try await session.respond(to: req.prompt, generating: ReplyOptions.self, options: options)
            sink.send(eventDone, json: WireDigest(replies: response.content.replies))
            return
        }
        var last = WireDigest(replies: [])
        let stream = session.streamResponse(to: req.prompt, generating: ReplyOptions.self, options: options)
        for try await snapshot in stream {
            try Task.checkCancellation()
            last = WireDigest(replies: snapshot.content.replies ?? [])
            sink.send(eventSnapshot, json: last)
        }
        try Task.checkCancellation()
        sink.send(eventDone, json: last)
    }

    /// One `MailQuery`, whole (it's small: no streaming).
    private static func query(_ session: LanguageModelSession, _ req: Request, _ options: GenerationOptions, _ sink: Sink) async throws {
        let q = try await session.respond(to: req.prompt, generating: MailQuery.self, options: options).content
        try Task.checkCancellation()
        sink.send(eventDone, json: WireQuery(
            subject: q.subject, op: q.op, measure: q.measure, groupBy: q.groupBy,
            timeframe: q.timeframe, compare: q.compare, place: q.place, merchant: q.merchant,
            person: q.person, direction: q.direction, field: q.field, tense: q.tense
        ))
    }

    private static func text(_ session: LanguageModelSession, _ req: Request, _ options: GenerationOptions, _ sink: Sink) async throws {
        if !req.stream {
            let response = try await session.respond(to: req.prompt, options: options)
            sink.send(eventDone, json: WireDigest(text: response.content))
            return
        }
        var last = ""
        let stream = session.streamResponse(to: req.prompt, options: options)
        for try await snapshot in stream {
            try Task.checkCancellation()
            last = snapshot.content
            sink.send(eventSnapshot, json: WireDigest(text: last))
        }
        try Task.checkCancellation()
        sink.send(eventDone, json: WireDigest(text: last))
    }

    private static func wire(_ d: ThreadDigest.PartiallyGenerated) -> WireDigest {
        WireDigest(
            points: d.points?.map { WireItem(source: $0.source, text: $0.text, due: nil) },
            asks: d.asks?.map { WireItem(source: $0.source, text: $0.text, due: $0.due) },
            gist: d.gist,
            text: nil
        )
    }

    /// With the macOS 26.4 token APIs: the exact size of this request, when
    /// it can't fit the window (instructions + prompt + schema + answer).
    /// Rust then re-plans smaller from the numbers instead of waiting for the
    /// model to fail.
    private static func overBudget(_ model: SystemLanguageModel, _ req: Request) async throws -> (Int, Int)? {
        #if compiler(>=6.3)
        if #available(macOS 26.4, *) {
            let limit = model.contextSize
            var used = try await model.tokenCount(for: req.instructions + "\n\n" + req.prompt)
            if req.isReplies {
                used += try await model.tokenCount(for: ReplyOptions.generationSchema)
            } else if req.isQuery {
                used += try await model.tokenCount(for: MailQuery.generationSchema)
            } else if req.guided {
                used += try await model.tokenCount(for: ThreadDigest.generationSchema)
            }
            let needed = used + req.maxResponseTokens
            return needed > limit ? (needed, limit) : nil
        }
        #endif
        return nil
    }

    /// An error as (code, tokens, limit) for Rust's `summary::engine`.
    static func describe(_ error: Error) -> (String, Int?, Int?) {
        if error is CancellationError { return ("cancelled", nil, nil) }
        #if compiler(>=6.4)
        if #available(macOS 27.0, *) {
            if let e = error as? LanguageModelError {
                switch e {
                case .contextSizeExceeded(let info): return ("contextExceeded", info.tokenCount, info.contextSize)
                case .guardrailViolation: return ("guardrail", nil, nil)
                case .refusal: return ("refusal", nil, nil)
                case .unsupportedLanguageOrLocale: return ("unsupportedLanguage", nil, nil)
                case .rateLimited: return ("rateLimited", nil, nil)
                case .timeout: return ("timeout", nil, nil)
                default: return ("other", nil, nil)
                }
            }
            if let e = error as? LanguageModelSession.Error {
                if case .concurrentRequests = e { return ("concurrentRequests", nil, nil) }
                return ("other", nil, nil)
            }
            if error is SystemLanguageModel.Error { return ("assetsUnavailable", nil, nil) }
        }
        #endif
        if let e = error as? LanguageModelSession.GenerationError {
            switch e {
            case .exceededContextWindowSize: return ("contextExceeded", nil, nil)
            case .guardrailViolation: return ("guardrail", nil, nil)
            case .refusal: return ("refusal", nil, nil)
            case .unsupportedLanguageOrLocale: return ("unsupportedLanguage", nil, nil)
            case .assetsUnavailable: return ("assetsUnavailable", nil, nil)
            case .rateLimited: return ("rateLimited", nil, nil)
            case .concurrentRequests: return ("concurrentRequests", nil, nil)
            case .decodingFailure: return ("decoding", nil, nil)
            default: return ("other", nil, nil)
            }
        }
        return ("other", nil, nil)
    }
}

#endif
