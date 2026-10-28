import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// One ordered connection. A new connection always initializes from snapshots; sends are never replayed.
public actor AHPClient {
    public let configuration: ClientConfiguration
    public var unsupportedEventCount: UInt64 { wire.unsupportedEventCount }
    public var duplicateEventCount: UInt64 { wire.duplicateEventCount }
    private let transport: HTTPTransport
    private var socket: ClientSocket?
    private var connectionID: ConnectionID?
    private var continuation: AsyncThrowingStream<ClientEvent, Error>.Continuation?
    private var receiver: Task<Void, Never>?
    private var expiry: Task<Void, Never>?
    private var outbound: Task<Void, Error>?
    private var clientSeq: UInt64 = 0
    private var wire = AHPProtocolState()
    private var timeouts: [RequestID: Task<Void, Never>] = [:]

    public init(configuration: ClientConfiguration) {
        self.configuration = configuration
        transport = HTTPTransport(session: ClientSession.make())
    }
    deinit {
        receiver?.cancel(); expiry?.cancel(); outbound?.cancel()
        for timeout in timeouts.values { timeout.cancel() }
        socket?.cancel()
        continuation?.finish()
        transport.session.invalidateAndCancel()
    }

    /// Consume immediately and wait for `.initialized` before sending. Errors finish the stream.
    /// Cancelling its consumer disconnects and cancels inference; it does not retry anything.
    public func connect(token: OAuthToken, subscriptions: [AHPChannel]) async throws -> AsyncThrowingStream<ClientEvent, Error> {
        guard socket == nil else { throw ClientError.alreadyConnected }
        guard subscriptions.count <= 32 else { throw ClientError.invalidInput }
        try Task.checkCancellation()
        var request = URLRequest(url: try configuration.webSocketURL())
        request.httpShouldHandleCookies = false
        request.setValue(try token.authorize(), forHTTPHeaderField: "Authorization")
        let id = ConnectionID(UUID())
        let stream = AsyncThrowingStream<ClientEvent, Error>(bufferingPolicy: .bufferingOldest(512)) { continuation in
            self.continuation = continuation
        }
        continuation?.onTermination = { [weak self] termination in
            guard case .cancelled = termination, let client = self else { return }
            Task { await client.close(id: id, error: nil) }
        }
        let task = ClientSocket(session: transport.session, request: request)
        socket = task; connectionID = id; clientSeq = 0
        wire = AHPProtocolState(subscriptions: Set(subscriptions))
        task.resume()
        emit(.connected(id))
        receiver = Task { [weak self] in
            do {
                while !Task.isCancelled {
                    let data = try await task.receive()
                    guard let self else { return }
                    try await self.receive(data, id: id)
                }
            } catch { await self?.close(id: id, error: (error as? ClientError) ?? .transport) }
        }
        let lifetime = max(0, min(token.expiresAt.timeIntervalSinceNow, 31_536_000))
        expiry = Task { [weak self] in
            do { try await Task.sleep(nanoseconds: UInt64(lifetime * 1_000_000_000)) } catch { return }
            await self?.close(id: id, error: .expiredToken)
        }
        do {
            try await command("initialize", params: InitializeParams(clientID: configuration.clientID,
                              initialSubscriptions: subscriptions), purpose: .initialize)
        } catch {
            close(id: id, error: (error as? ClientError) ?? .transport)
            throw (error as? ClientError) ?? ClientError.transport
        }
        return stream
    }

    public func disconnect() { if let id = connectionID { close(id: id, error: nil) } }

    /// Completion is represented by the corresponding snapshot on the event stream.
    @discardableResult
    public func subscribe(_ channel: AHPChannel) async throws -> RequestID {
        guard wire.initialized, connectionID != nil else { throw ClientError.notInitialized }
        guard wire.reservedSubscriptions.contains(channel) || wire.reservedSubscriptions.count < 32 else { throw ClientError.invalidInput }
        return try await command("subscribe", params: SubscriptionParams(channel: channel), purpose: .subscribe(channel))
    }

    /// The receipt identifies the transmission, not acceptance. Match the action's origin/clientSeq
    /// and inspect rejectionReason. On an uncertain outcome, refresh; never automatically resend.
    public func send(text: String, to chat: ChatURI, turnID: UUID = UUID(), at date: Date = Date()) async throws -> DispatchReceipt {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              text.utf8.count <= 32 * 1024, !text.contains("\0"),
              turnID != UUID(uuid: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) else {
            throw ClientError.invalidInput
        }
        let receipt = try nextReceipt(chat: chat, turnID: TurnID(turnID.uuidString.lowercased()))
        let timestamp = ISO8601DateFormatter().string(from: date)
        try await write(RPCRequest(id: nil, method: "dispatchAction", params: StartParams(channel: chat,
            clientSeq: receipt.clientSeq, action: StartAction(turnID: receipt.turnID, startedAt: timestamp,
                                                            message: UserMessage(text: text)))))
        return receipt
    }
    public func cancel(turnID: TurnID, in chat: ChatURI) async throws -> DispatchReceipt {
        guard let uuid = UUID(uuidString: turnID.rawValue), uuid.uuidString.lowercased() == turnID.rawValue,
              uuid != UUID(uuid: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) else { throw ClientError.invalidInput }
        let receipt = try nextReceipt(chat: chat, turnID: turnID)
        try await write(RPCRequest(id: nil, method: "dispatchAction", params: CancelParams(channel: chat,
            clientSeq: receipt.clientSeq, action: CancelAction(turnID: turnID))))
        return receipt
    }

    private func nextReceipt(chat: ChatURI, turnID: TurnID) throws -> DispatchReceipt {
        guard wire.initialized, let id = connectionID else { throw ClientError.notInitialized }
        guard wire.subscriptions.contains(.chat(chat)), clientSeq < 9_007_199_254_740_991 else { throw ClientError.invalidInput }
        clientSeq += 1
        return DispatchReceipt(connectionID: id, clientSeq: clientSeq, turnID: turnID)
    }
    @discardableResult
    private func command<Params: Encodable>(_ method: String, params: Params, purpose: AHPRequestPurpose) async throws -> RequestID {
        guard let connection = connectionID else { throw ClientError.disconnected }
        let requestID = RequestID(UUID())
        timeouts[requestID] = Task { [weak self] in
            do { try await Task.sleep(nanoseconds: 30_000_000_000) } catch { return }
            await self?.close(id: connection, error: .timeout)
        }
        wire.register(requestID, purpose: purpose)
        try await write(RPCRequest(id: requestID, method: method, params: params))
        return requestID
    }
    private func write<Value: Encodable>(_ value: Value) async throws {
        guard let socket, let id = connectionID else { throw ClientError.disconnected }
        let data = try JSONEncoder().encode(value)
        guard data.count <= 64 * 1024, let text = String(data: data, encoding: .utf8) else { throw ClientError.invalidInput }
        let previous = outbound
        let next = Task {
            if let previous { try await previous.value }
            try Task.checkCancellation()
            try await socket.send(text)
        }
        outbound = next
        do {
            try await withTaskCancellationHandler { try await next.value } onCancel: { next.cancel(); socket.cancel() }
            try Task.checkCancellation()
        } catch {
            let safe: ClientError = error is CancellationError || Task.isCancelled ? .cancelled : .transport
            close(id: id, error: safe)
            throw safe
        }
    }
    private func receive(_ data: Data, id: ConnectionID) throws {
        guard id == connectionID else { return }
        let result = try wire.process(data)
        if let completed = result.completedRequestID { timeouts.removeValue(forKey: completed)?.cancel() }
        for event in result.events { emit(event) }
    }
    private func emit(_ event: ClientEvent) {
        guard let continuation else { return }
        if case .dropped = continuation.yield(event), let id = connectionID { close(id: id, error: .eventBufferOverflow) }
    }
    private func close(id: ConnectionID, error: ClientError?) {
        guard id == connectionID else { return }
        connectionID = nil
        receiver?.cancel(); receiver = nil
        expiry?.cancel(); expiry = nil
        outbound?.cancel(); outbound = nil
        for timeout in timeouts.values { timeout.cancel() }
        timeouts.removeAll()
        socket?.cancel(); socket = nil
        continuation?.yield(.disconnected)
        if let error { continuation?.finish(throwing: error) } else { continuation?.finish() }
        continuation = nil
    }
}
