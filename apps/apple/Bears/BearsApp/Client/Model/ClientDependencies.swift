import Foundation
import BearsClient

protocol NativeAPI: Sendable {
    func bears(token: OAuthToken) async throws -> [BearSummary]
    func conversations(bearID: BearID, token: OAuthToken) async throws -> ConversationInventory
    func createConversation(_ input: CreateConversation, token: OAuthToken) async throws -> CreatedConversation
    func history(bearID: BearID, conversationID: ConversationID, before: Int64?, limit: Int, token: OAuthToken) async throws -> HistoryPage
    func userInfo(token: OAuthToken) async throws -> UserInfo
    func exchangeCode(_ code: AuthorizationCode, authorization: OAuthAuthorization) async throws -> OAuthToken
}
extension DenClient: NativeAPI {}

protocol NativeSocket: Sendable {
    func connect(token: OAuthToken, subscriptions: [AHPChannel]) async throws -> AsyncThrowingStream<ClientEvent, Error>
    func send(text: String, to chat: ChatURI, turnID: UUID, at date: Date) async throws -> DispatchReceipt
    func cancel(turnID: TurnID, in chat: ChatURI) async throws -> DispatchReceipt
    func disconnect() async
}
extension AHPClient: NativeSocket {}

@MainActor
struct ClientDependencies {
    let preferences: ClientPreferences
    let vault: any TokenVault
    let browser: any OAuthBrowser
    let makeAPI: @Sendable (ClientConfiguration) -> any NativeAPI
    let makeSocket: @Sendable (ClientConfiguration) -> any NativeSocket
    let loginMaterial: @Sendable () throws -> LoginMaterial

    static func live() -> Self {
        Self(preferences: ClientPreferences(), vault: KeychainTokenVault(), browser: SystemOAuthBrowser(),
             makeAPI: { DenClient(configuration: $0) }, makeSocket: { AHPClient(configuration: $0) },
             loginMaterial: { try PKCEGenerator.generate() })
    }
}

struct OperationGeneration: Equatable, Sendable {
    private let value = UUID()
}

struct NativeConversation: Identifiable, Equatable, Sendable {
    let id: ConversationID
    var title: String
    let hatID: HatID?
    var canSend: Bool
    let sessionURI: SessionURI
    let chatURI: ChatURI
    init(_ value: ConversationSummary) {
        id = value.id; title = value.title; hatID = value.hatID; canSend = value.canSend
        sessionURI = value.sessionURI; chatURI = value.chatURI
    }
    init(_ value: CreatedConversation) {
        id = value.id; title = value.title; hatID = value.hatID; canSend = true
        sessionURI = value.sessionURI; chatURI = value.chatURI
    }
}

enum ClientAuthentication: Equatable {
    case signedOut, signingIn, signedIn, needsReauthorization
}
enum ChatConnection: Equatable {
    case disconnected, connecting, ready
    var label: String {
        switch self {
        case .disconnected: return "Disconnected"
        case .connecting: return "Connecting…"
        case .ready: return "Live"
        }
    }
}
