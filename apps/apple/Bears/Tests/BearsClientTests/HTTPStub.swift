import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Mutable state is confined to a lock; each test gets an isolated hostname/handler.
final class HTTPStubRegistry: @unchecked Sendable {
    typealias Handler = @Sendable (URLRequest) throws -> (Int, Data)
    private let lock = NSLock()
    private var handlers: [String: Handler] = [:]
    func register(host: String, handler: @escaping Handler) {
        lock.lock(); defer { lock.unlock() }
        handlers[host] = handler
    }
    func remove(host: String) {
        lock.lock(); defer { lock.unlock() }
        handlers.removeValue(forKey: host)
    }
    func handler(for host: String) -> Handler? {
        lock.lock(); defer { lock.unlock() }
        return handlers[host]
    }
}

final class HTTPStub: URLProtocol, @unchecked Sendable {
    static let registry = HTTPStubRegistry()
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        do {
            guard let host = request.url?.host, let handler = Self.registry.handler(for: host) else {
                throw URLError(.badURL)
            }
            let (status, data) = try handler(request)
            let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
                                           headerFields: ["Content-Type": "application/json"])!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch { client?.urlProtocol(self, didFailWithError: error) }
    }
    override func stopLoading() {}
}
