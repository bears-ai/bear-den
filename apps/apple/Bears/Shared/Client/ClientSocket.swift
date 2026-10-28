import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// URLSession tasks support concurrent send/receive/cancel. Linux 5.10 lacks Apple's Sendable annotations.
/// The wrapper owns an immutable task reference and never exposes the unannotated Message across actors.
final class ClientSocket: @unchecked Sendable {
    private let task: URLSessionWebSocketTask
    init(session: URLSession, request: URLRequest) {
        task = session.webSocketTask(with: request)
        task.maximumMessageSize = 2 * 1024 * 1024
    }
    func resume() { task.resume() }
    func cancel() { task.cancel(with: .goingAway, reason: nil) }
    func send(_ text: String) async throws { try await task.send(.string(text)) }
    func receive() async throws -> Data {
        switch try await task.receive() {
        case .string(let text): return Data(text.utf8)
        case .data: throw ClientError.invalidResponse
        #if !os(Linux)
        @unknown default: throw ClientError.invalidResponse
        #endif
        }
    }
    deinit { cancel() }
}
