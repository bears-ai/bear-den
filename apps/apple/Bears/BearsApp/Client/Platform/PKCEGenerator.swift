import Foundation
import CryptoKit
import Security
import BearsClient

struct LoginMaterial: Sendable {
    let pkce: PKCEMaterial
    let state: String
}

enum PKCEGenerator {
    static func generate() throws -> LoginMaterial {
        let verifier = try randomBytes().base64URL
        let challenge = Data(SHA256.hash(data: Data(verifier.utf8))).base64URL
        return try LoginMaterial(pkce: PKCEMaterial(verifier: verifier, challenge: challenge),
                                 state: randomBytes().base64URL)
    }
    private static func randomBytes() throws -> Data {
        var bytes = [UInt8](repeating: 0, count: 32)
        let status = bytes.withUnsafeMutableBytes { buffer in
            SecRandomCopyBytes(kSecRandomDefault, buffer.count, buffer.baseAddress!)
        }
        guard status == errSecSuccess else { throw ClientPlatformError.randomGeneration }
        return Data(bytes)
    }
}

private extension Data {
    var base64URL: String {
        base64EncodedString().replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }
}

enum ClientPlatformError: Error, LocalizedError, Equatable, Sendable {
    case randomGeneration, browserUnavailable, noPresentationWindow, signInCancelled
    case keychain(Int32), invalidStoredToken

    var errorDescription: String? {
        switch self {
        case .randomGeneration: return "Secure login material could not be generated."
        case .browserUnavailable: return "The system authentication browser could not be started."
        case .noPresentationWindow: return "Open the Bears window before signing in."
        case .signInCancelled: return "Sign-in was cancelled."
        case .keychain(let status): return "Keychain access failed (status \(status)). No credential was written to preferences."
        case .invalidStoredToken: return "The saved credential could not be read. Sign out to remove it, then sign in again."
        }
    }
}
