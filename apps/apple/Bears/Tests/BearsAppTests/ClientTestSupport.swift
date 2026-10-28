import Foundation
import XCTest
@testable import BearsClient
@testable import BearsApp

enum NativeFixtures {
    static let bearA = BearID(UUID(uuidString: "11111111-1111-4111-8111-111111111111")!)
    static let bearB = BearID(UUID(uuidString: "22222222-2222-4222-8222-222222222222")!)
    static let hat = HatID(UUID(uuidString: "33333333-3333-4333-8333-333333333333")!)
    static let chatA = try! ChatURI(UUID(uuidString: "44444444-4444-4444-8444-444444444444")!)
    static let chatB = try! ChatURI(UUID(uuidString: "55555555-5555-4555-8555-555555555555")!)
    static let connection = ConnectionID(UUID(uuidString: "66666666-6666-4666-8666-666666666666")!)
    static let turn = UUID(uuidString: "77777777-7777-4777-8777-777777777777")!
    static var token: OAuthToken {
        OAuthToken(accessToken: try! AccessToken("test-only-sensitive-token"), expiresAt: Date().addingTimeInterval(3600), scope: "data:read data:write")
    }
    static func configuration(_ path: String = "den", client: String = "apple-test") throws -> ClientConfiguration {
        try ClientConfiguration(baseURL: URL(string: "https://test.example/\(path)")!, clientID: client)
    }
    static func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T { try JSONDecoder().decode(type, from: Data(json.utf8)) }
    static func bears() throws -> [BearSummary] {
        try decode([BearSummary].self, """
        [{"bear_id":"\(bearA.rawValue)","slug":"a","name":"Bear A","description":"Purpose A","is_bear_admin":true},
         {"bear_id":"\(bearB.rawValue)","slug":"b","name":"Bear B","description":"Purpose B","is_bear_admin":false}]
        """)
    }
    static func conversation(_ chat: ChatURI = chatA) throws -> NativeConversation {
        NativeConversation(try decode(ConversationSummary.self, """
        {"id":"conv-\(chat.conversationUUID)","title":"Chat","hat_id":"\(hat.rawValue)","can_send":true,
         "session_uri":"ahp-session:/\(chat.conversationUUID.uuidString.lowercased())","chat_uri":"\(chat.rawValue)"}
        """))
    }
    static func created() throws -> CreatedConversation {
        try decode(CreatedConversation.self, """
        {"id":"conv-\(chatA.conversationUUID)","title":"Chat","hat_id":"\(hat.rawValue)",
         "session_uri":"ahp-session:/\(chatA.conversationUUID.uuidString.lowercased())","chat_uri":"\(chatA.rawValue)"}
        """)
    }
    static func inventory(_ title: String = "Hat") throws -> ConversationInventory {
        try decode(ConversationInventory.self, """
        {"conversations":[{"id":"conv-\(chatA.conversationUUID)","title":"Chat","hat_id":"\(hat.rawValue)","can_send":true,
         "session_uri":"ahp-session:/\(chatA.conversationUUID.uuidString.lowercased())","chat_uri":"\(chatA.rawValue)"}],
         "hats":[{"id":"\(hat.rawValue)","name":"\(title)","short_summary":"Shared summary"}]}
        """)
    }
    static func history() throws -> HistoryPage { try decode(HistoryPage.self, "{\"messages\":[],\"has_more\":false,\"next_before\":null}") }
    static func profile() throws -> UserInfo { try decode(UserInfo.self, "{\"sub\":\"1\",\"name\":\"Human\"}") }
    static func action(turn: TurnID, seq: UInt64 = 1, clientID: String = "apple-test", rejection: Bool = false,
                       type: String = "chat/turnStarted") throws -> AHPActionEnvelope {
        let payload: String
        if type == "chat/turnStarted" {
            payload = "\"startedAt\":\"2026-10-28T00:00:00Z\",\"message\":{\"text\":\"Draft\",\"origin\":{\"kind\":\"user\"}}"
        } else { payload = "\"duration\":1" }
        let reason = rejection ? ",\"rejectionReason\":\"Rejected\"" : ""
        return try decode(AHPActionEnvelope.self, """
        {"channel":"\(chatA.rawValue)","serverSeq":12,"origin":{"clientId":"\(clientID)","clientSeq":\(seq)},
         "action":{"type":"\(type)","turnId":"\(turn.rawValue)",\(payload)}\(reason)}
        """)
    }
    static func snapshot(_ chat: ChatURI) throws -> ChatSnapshot {
        let wire = try decode(WireSnapshot.self, """
        {"resource":"\(chat.rawValue)","fromSeq":10,"state":{"resource":"\(chat.rawValue)","title":"Snapshot chat",
         "status":1,"modifiedAt":"2026-10-28T00:00:00Z","interactivity":"full","turns":[]}}
        """)
        guard case .chat(let snapshot) = wire else { throw ClientError.invalidResponse }
        return snapshot
    }
}

@MainActor
final class TestVault: TokenVault {
    var token: OAuthToken?
    var saved: [OAuthToken] = []
    var removed = 0
    var failRemoval = false
    var failSave = false
    func load(for configuration: ClientConfiguration) throws -> OAuthToken? { token }
    func save(_ token: OAuthToken, for configuration: ClientConfiguration) throws {
        if failSave { throw ClientPlatformError.keychain(-2) }
        saved.append(token); self.token = token
    }
    func remove(for configuration: ClientConfiguration) throws {
        removed += 1
        if failRemoval { throw ClientPlatformError.keychain(-1) }
        token = nil
    }
}
@MainActor
final class TestBrowser: OAuthBrowser {
    func authorize(_ authorization: OAuthAuthorization) async throws -> AuthorizationCode {
        var parts = URLComponents(url: authorization.redirectURI, resolvingAgainstBaseURL: false)!
        parts.queryItems = [URLQueryItem(name: "state", value: authorization.state), URLQueryItem(name: "code", value: "test-code")]
        return try authorization.validateCallback(parts.url!)
    }
    func cancel() {}
}

actor TestNativeAPI: NativeAPI {
    var delayInventory = false
    var delayExchange = false
    var pendingInventory: [BearID: CheckedContinuation<ConversationInventory, Error>] = [:]
    var pendingExchange: CheckedContinuation<OAuthToken, Error>?
    var creationRequests: [CreateConversation] = []
    func bears(token: OAuthToken) async throws -> [BearSummary] { [] }
    func conversations(bearID: BearID, token: OAuthToken) async throws -> ConversationInventory {
        if delayInventory { return try await withCheckedThrowingContinuation { pendingInventory[bearID] = $0 } }
        return try NativeFixtures.inventory()
    }
    func createConversation(_ input: CreateConversation, token: OAuthToken) async throws -> CreatedConversation {
        creationRequests.append(input)
        if creationRequests.count == 1 { throw ClientError.transport }
        return try NativeFixtures.created()
    }
    func history(bearID: BearID, conversationID: ConversationID, before: Int64?, limit: Int, token: OAuthToken) async throws -> HistoryPage { try NativeFixtures.history() }
    func userInfo(token: OAuthToken) async throws -> UserInfo { try NativeFixtures.profile() }
    func exchangeCode(_ code: AuthorizationCode, authorization: OAuthAuthorization) async throws -> OAuthToken {
        if delayExchange { return try await withCheckedThrowingContinuation { pendingExchange = $0 } }
        return NativeFixtures.token
    }
    func suspendInventory() { delayInventory = true }
    func suspendExchange() { delayExchange = true }
    func hasInventory(_ id: BearID) -> Bool { pendingInventory[id] != nil }
    func hasExchange() -> Bool { pendingExchange != nil }
    func finishInventory(_ id: BearID, title: String) throws {
        pendingInventory.removeValue(forKey: id)?.resume(returning: try NativeFixtures.inventory(title))
    }
    func finishExchange() { pendingExchange?.resume(returning: NativeFixtures.token); pendingExchange = nil }
}

actor TestNativeSocket: NativeSocket {
    let id: ConnectionID
    private var continuation: AsyncThrowingStream<ClientEvent, Error>.Continuation?
    var disconnects = 0
    private var delayConnect = false
    private var pendingConnect: CheckedContinuation<Void, Never>?
    init(id: ConnectionID = ConnectionID(UUID())) { self.id = id }
    func suspendConnect() { delayConnect = true }
    func isConnectPending() -> Bool { pendingConnect != nil }
    func finishConnect() { pendingConnect?.resume(); pendingConnect = nil }
    func connect(token: OAuthToken, subscriptions: [AHPChannel]) async throws -> AsyncThrowingStream<ClientEvent, Error> {
        if delayConnect { await withCheckedContinuation { pendingConnect = $0 } }
        let stream = AsyncThrowingStream<ClientEvent, Error> { continuation = $0 }
        continuation?.yield(.connected(id))
        for channel in subscriptions {
            if case .chat(let chat) = channel { continuation?.yield(.snapshot(try NativeFixtures.snapshot(chat))) }
        }
        continuation?.yield(.initialized(protocolVersion: "1.0.0"))
        return stream
    }
    func send(text: String, to chat: ChatURI, turnID: UUID, at date: Date) async throws -> DispatchReceipt {
        DispatchReceipt(connectionID: id, clientSeq: 1, turnID: TurnID(turnID.uuidString.lowercased()))
    }
    func cancel(turnID: TurnID, in chat: ChatURI) async throws -> DispatchReceipt {
        DispatchReceipt(connectionID: id, clientSeq: 2, turnID: turnID)
    }
    func disconnect() { disconnects += 1; continuation?.finish(); continuation = nil }
    func emit(_ event: ClientEvent) { continuation?.yield(event) }
}

final class TestSocketFactory: @unchecked Sendable {
    private let lock = NSLock()
    private var sockets: [TestNativeSocket]
    init(_ sockets: [TestNativeSocket]) { self.sockets = sockets }
    func next() -> any NativeSocket {
        lock.lock(); defer { lock.unlock() }
        return sockets.isEmpty ? TestNativeSocket() : sockets.removeFirst()
    }
}

@MainActor
func makeClientModel(api: TestNativeAPI = TestNativeAPI(), vault: TestVault? = nil,
                     sockets: [TestNativeSocket] = []) -> NativeClientModel {
    let defaults = UserDefaults(suiteName: "BearsClientTests.\(UUID())")!
    let factory = TestSocketFactory(sockets)
    let dependencies = ClientDependencies(preferences: ClientPreferences(defaults: defaults), vault: vault ?? TestVault(),
        browser: TestBrowser(), makeAPI: { _ in api }, makeSocket: { _ in factory.next() }, loginMaterial: {
            try LoginMaterial(pkce: PKCEMaterial(verifier: String(repeating: "v", count: 43), challenge: String(repeating: "c", count: 43)), state: "test-state")
        })
    let model = NativeClientModel(dependencies: dependencies)
    model.serverURLText = "https://test.example/den"
    model.publicClientIDText = "apple-test"
    model.configure()
    return model
}

@MainActor
func waitUntil(_ condition: @MainActor () async -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
    for _ in 0..<100 {
        if await condition() { return }
        try? await Task.sleep(nanoseconds: 2_000_000)
    }
    XCTFail("Asynchronous test condition timed out", file: file, line: line)
}
