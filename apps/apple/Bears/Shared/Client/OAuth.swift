@preconcurrency import Foundation

public enum OAuthScope: String, Codable, Sendable {
    case profileRead = "profile:read", profileEmail = "profile:email"
    case dataRead = "data:read", dataWrite = "data:write"
}

public struct AccessToken: Codable, Sendable, Equatable, CustomStringConvertible, CustomDebugStringConvertible {
    public let rawValue: String
    public var description: String { "AccessToken(<redacted>)" }
    public var debugDescription: String { description }
    public init(_ value: String) throws {
        guard !value.isEmpty, value.utf8.count <= 16_384,
              value.utf8.allSatisfy({ $0 > 32 && $0 < 127 }) else { throw ClientError.invalidInput }
        rawValue = value
    }
    public init(from decoder: Decoder) throws { try self.init(decoder.singleValueContainer().decode(String.self)) }
    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

public struct OAuthToken: Codable, Sendable, Equatable, CustomStringConvertible, CustomDebugStringConvertible {
    public let accessToken: AccessToken
    public let expiresAt: Date
    public let scope: String
    public var description: String { "OAuthToken(<redacted>)" }
    public var debugDescription: String { description }
    public init(accessToken: AccessToken, expiresAt: Date, scope: String) {
        self.accessToken = accessToken
        self.expiresAt = expiresAt
        self.scope = scope
    }
    public func isExpired(at date: Date = Date()) -> Bool { date >= expiresAt }
    func authorize(at date: Date = Date()) throws -> String {
        guard !isExpired(at: date) else { throw ClientError.expiredToken }
        return "Bearer " + accessToken.rawValue
    }
}

/// Generate a random verifier and its base64url SHA-256 challenge outside this module.
/// This type validates their wire syntax; it does not claim to verify the digest relationship.
public struct PKCEMaterial: Sendable, CustomStringConvertible, CustomDebugStringConvertible {
    public let verifier: String
    public let challenge: String
    public var description: String { "PKCEMaterial(<redacted>)" }
    public var debugDescription: String { description }
    public init(verifier: String, challenge: String) throws {
        let verifierCharacters = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~".utf8)
        let challengeCharacters = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_".utf8)
        guard (43...128).contains(verifier.utf8.count), verifier.utf8.allSatisfy(verifierCharacters.contains),
              challenge.utf8.count == 43, challenge.utf8.allSatisfy(challengeCharacters.contains) else {
            throw ClientError.invalidInput
        }
        self.verifier = verifier
        self.challenge = challenge
    }
}

public struct AuthorizationCode: Sendable, CustomStringConvertible, CustomDebugStringConvertible {
    public let rawValue: String
    public var description: String { "AuthorizationCode(<redacted>)" }
    public var debugDescription: String { description }
    // Only a validated callback can produce a code for exchange.
    init(_ value: String) { rawValue = value }
}

public struct OAuthAuthorization: Sendable, CustomStringConvertible, CustomDebugStringConvertible {
    public let configuration: ClientConfiguration
    public let redirectURI: URL
    public let state: String
    public let pkce: PKCEMaterial
    public let scopes: [OAuthScope]
    public var description: String { "OAuthAuthorization(<redacted>)" }
    public var debugDescription: String { description }

    /// `state` must be freshly generated with a cryptographic RNG by the platform layer.
    public init(configuration: ClientConfiguration, redirectURI: URL, state: String, pkce: PKCEMaterial,
                scopes: [OAuthScope] = [.profileRead, .dataRead, .dataWrite]) throws {
        guard let redirect = URLComponents(url: redirectURI, resolvingAgainstBaseURL: false),
              let scheme = redirect.scheme, !scheme.isEmpty,
              redirect.user == nil, redirect.password == nil, redirect.query == nil, redirect.fragment == nil,
              !state.isEmpty, state.utf8.count <= 512,
              !state.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw ClientError.invalidInput
        }
        if scheme == "http" || scheme == "https" {
            guard let host = redirect.host,
                  scheme == "https" || ["localhost", "127.0.0.1"].contains(host) else { throw ClientError.invalidInput }
        }
        self.configuration = configuration
        self.redirectURI = redirectURI
        self.state = state
        self.pkce = pkce
        self.scopes = scopes
    }

    public func authorizationURL() throws -> URL {
        try configuration.endpoint("oauth/authorize", query: [
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "client_id", value: configuration.clientID),
            URLQueryItem(name: "redirect_uri", value: redirectURI.absoluteString),
            URLQueryItem(name: "scope", value: scopes.map(\.rawValue).joined(separator: " ")),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: pkce.challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256")
        ])
    }

    /// Checks destination and state before inspecting either success or error parameters.
    public func validateCallback(_ url: URL) throws -> AuthorizationCode {
        guard url.absoluteString.utf8.count <= 16_384,
              let actual = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let expected = URLComponents(url: redirectURI, resolvingAgainstBaseURL: false),
              actual.scheme == expected.scheme, actual.host == expected.host, actual.port == expected.port,
              actual.percentEncodedPath == expected.percentEncodedPath,
              actual.user == nil, actual.password == nil, actual.fragment == nil else {
            throw ClientError.invalidCallback
        }
        let items = actual.queryItems ?? []
        func values(_ key: String) -> [URLQueryItem] { items.filter { $0.name == key } }
        guard values("state").count == 1, values("state").first?.value == state,
              values("code").count <= 1, values("error").count <= 1 else { throw ClientError.invalidCallback }
        if !values("error").isEmpty {
            guard values("code").isEmpty, let error = values("error").first?.value, !error.isEmpty else {
                throw ClientError.invalidCallback
            }
            throw ClientError.authorizationDenied
        }
        guard let code = values("code").first?.value, !code.isEmpty,
              !code.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw ClientError.invalidCallback
        }
        return AuthorizationCode(code)
    }
}

struct TokenResponse: Decodable, Sendable {
    let accessToken: AccessToken
    let tokenType: String
    let expiresIn: Int64
    let scope: String
    enum CodingKeys: String, CodingKey {
        case accessToken = "access_token", tokenType = "token_type", expiresIn = "expires_in", scope
    }
    func token(issuedAt: Date) throws -> OAuthToken {
        guard tokenType.caseInsensitiveCompare("Bearer") == .orderedSame,
              expiresIn > 0, expiresIn <= 31_536_000 else { throw ClientError.invalidResponse }
        return OAuthToken(accessToken: accessToken, expiresAt: issuedAt.addingTimeInterval(Double(expiresIn)), scope: scope)
    }
}
