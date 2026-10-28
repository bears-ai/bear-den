@preconcurrency import Foundation

public struct ClientConfiguration: Sendable, Equatable {
    public let baseURL: URL
    public let clientID: String

    /// The supplied URL is a deployment root, optionally including a path prefix.
    public init(baseURL: URL, clientID: String) throws {
        guard var parts = URLComponents(url: baseURL, resolvingAgainstBaseURL: false),
              let host = parts.host, !host.isEmpty,
              parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil,
              parts.scheme == "https" || (parts.scheme == "http" && ["localhost", "127.0.0.1"].contains(host)),
              !clientID.isEmpty, clientID.utf8.count <= 128,
              !clientID.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw ClientError.invalidConfiguration
        }
        // A prefix is a directory even if the user's URL did not end in a slash.
        if !parts.path.hasSuffix("/") { parts.path += "/" }
        guard let normalized = parts.url else { throw ClientError.invalidConfiguration }
        self.baseURL = normalized
        self.clientID = clientID
    }

    func endpoint(_ path: String, query: [URLQueryItem] = []) throws -> URL {
        guard let url = URL(string: path, relativeTo: baseURL)?.absoluteURL,
              var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            throw ClientError.invalidConfiguration
        }
        if !query.isEmpty {
            parts.queryItems = query
            // HTTP query parsers commonly treat a literal plus as a form-encoded space.
            parts.percentEncodedQuery = parts.percentEncodedQuery?.replacingOccurrences(of: "+", with: "%2B")
        }
        guard let result = parts.url else { throw ClientError.invalidConfiguration }
        return result
    }

    func webSocketURL() throws -> URL {
        var parts = URLComponents(url: try endpoint("v1.0/agent-host"), resolvingAgainstBaseURL: false)!
        parts.scheme = parts.scheme == "https" ? "wss" : "ws"
        guard let url = parts.url else { throw ClientError.invalidConfiguration }
        return url
    }
}

/// No server body, URL, credential, or underlying transport description is retained.
public enum ClientError: Error, Sendable, Equatable, LocalizedError {
    case invalidConfiguration, invalidInput, invalidCallback, authorizationDenied
    case expiredToken, invalidResponse, responseTooLarge, transport, cancelled
    case http(status: Int, code: ServerErrorCode)
    case rpc(code: Int)
    case disconnected, alreadyConnected, notInitialized, timeout, eventBufferOverflow

    public var errorDescription: String? {
        switch self {
        case .invalidConfiguration: return "Use an HTTPS deployment URL and a valid public client ID."
        case .invalidInput: return "The client input is invalid."
        case .invalidCallback: return "The authorization callback did not match this login request."
        case .authorizationDenied: return "Authorization was not granted."
        case .expiredToken: return "Sign in again; this access token has expired."
        case .invalidResponse: return "The server response was not supported."
        case .responseTooLarge: return "The server response exceeded the client limit."
        case .transport: return "The connection failed. Refresh canonical history before retrying a send."
        case .cancelled: return "The operation was cancelled."
        case .http(let status, _): return "The server rejected the request (HTTP \(status))."
        case .rpc(let code): return "The server rejected the protocol request (\(code))."
        case .disconnected: return "The connection is closed. Refresh before sending again."
        case .alreadyConnected: return "Disconnect the existing connection first."
        case .notInitialized: return "The connection is not initialized."
        case .timeout: return "The protocol request timed out."
        case .eventBufferOverflow: return "The event consumer fell behind. Reconnect for a fresh snapshot."
        }
    }
}

public enum ServerErrorCode: String, Codable, Sendable {
    case invalidRequest = "invalid_request"
    case conversationUnavailable = "conversation_unavailable"
    case conflict, forbidden, unauthorized
    case invalidToken = "invalid_token"
    case invalidGrant = "invalid_grant"
    case invalidClient = "invalid_client"
    case insufficientScope = "insufficient_scope"
    case modelUnavailable = "model_unavailable"
    case serviceUnavailable = "service_unavailable"
    case unknown
}
