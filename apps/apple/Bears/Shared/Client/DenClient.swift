import Foundation
#if canImport(FoundationNetworking)
@preconcurrency import FoundationNetworking
#endif

/// Stateless authentication: the platform owns the token and supplies it for each operation.
/// Requests are never retried automatically, including conversation creation and token exchange.
public actor DenClient {
    public let configuration: ClientConfiguration
    private let transport: HTTPTransport

    public init(configuration: ClientConfiguration) {
        self.configuration = configuration
        transport = HTTPTransport(session: ClientSession.make())
    }
    init(configuration: ClientConfiguration, protocolClasses: [AnyClass]) {
        self.configuration = configuration
        transport = HTTPTransport(session: ClientSession.make(protocolClasses: protocolClasses))
    }
    deinit { transport.session.invalidateAndCancel() }

    public func bears(token: OAuthToken) async throws -> [BearSummary] {
        try await get("v1.0/bears", token: token)
    }
    public func conversations(bearID: BearID, token: OAuthToken) async throws -> ConversationInventory {
        try await get("v1.0/chat/conversations", query: [URLQueryItem(name: "bear_id", value: bearID.rawValue.uuidString.lowercased())], token: token)
    }
    public func createConversation(_ input: CreateConversation, token: OAuthToken) async throws -> CreatedConversation {
        var request = try request("v1.0/chat/conversations", token: token)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(input)
        return try await transport.execute(request)
    }
    public func history(bearID: BearID, conversationID: ConversationID, before: Int64? = nil,
                        limit: Int = 50, token: OAuthToken) async throws -> HistoryPage {
        guard (1...100).contains(limit), before.map({ $0 > 0 }) ?? true,
              !conversationID.rawValue.isEmpty else { throw ClientError.invalidInput }
        var query = [URLQueryItem(name: "bear_id", value: bearID.rawValue.uuidString.lowercased()),
                     URLQueryItem(name: "conversation_id", value: conversationID.rawValue),
                     URLQueryItem(name: "limit", value: String(limit))]
        if let before { query.append(URLQueryItem(name: "before", value: String(before))) }
        return try await get("v1.0/chat/history", query: query, token: token)
    }
    public func userInfo(token: OAuthToken) async throws -> UserInfo { try await get("oauth/userinfo", token: token) }

    /// Call only with the code returned by this authorization's validated callback.
    public func exchangeCode(_ code: AuthorizationCode, authorization: OAuthAuthorization) async throws -> OAuthToken {
        guard authorization.configuration == configuration else { throw ClientError.invalidConfiguration }
        var request = try request("oauth/token", token: nil)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        request.httpBody = formEncoded([
            ("grant_type", "authorization_code"), ("client_id", configuration.clientID),
            ("code", code.rawValue), ("redirect_uri", authorization.redirectURI.absoluteString),
            ("code_verifier", authorization.pkce.verifier)
        ])
        // Start time errs toward early expiry, not extending lifetime by network latency.
        let issuedAt = Date()
        let response: TokenResponse = try await transport.execute(request)
        return try response.token(issuedAt: issuedAt)
    }

    private func get<Response: Decodable & Sendable>(_ path: String, query: [URLQueryItem] = [], token: OAuthToken) async throws -> Response {
        try await transport.execute(request(path, query: query, token: token))
    }
    private func request(_ path: String, query: [URLQueryItem] = [], token: OAuthToken?) throws -> URLRequest {
        var request = URLRequest(url: try configuration.endpoint(path, query: query))
        request.httpShouldHandleCookies = false
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let token { request.setValue(try token.authorize(), forHTTPHeaderField: "Authorization") }
        return request
    }
}
