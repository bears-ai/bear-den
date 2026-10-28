import Foundation

/// Request/notification interpretation is independent of the transport and has one sequence owner.
enum AHPRequestPurpose { case initialize, subscribe(AHPChannel) }
struct AHPProcessedFrame {
    let completedRequestID: RequestID?
    let events: [ClientEvent]
}
struct AHPProtocolState {
    private(set) var initialized = false
    private(set) var serverSeq: UInt64 = 0
    private(set) var unsupportedEventCount: UInt64 = 0
    private(set) var duplicateEventCount: UInt64 = 0
    private(set) var subscriptions: Set<AHPChannel>
    private var pending: [RequestID: AHPRequestPurpose] = [:]

    init(subscriptions: Set<AHPChannel> = []) { self.subscriptions = subscriptions }
    var reservedSubscriptions: Set<AHPChannel> {
        subscriptions.union(pending.values.compactMap { if case .subscribe(let channel) = $0 { return channel }; return nil })
    }
    mutating func register(_ id: RequestID, purpose: AHPRequestPurpose) { pending[id] = purpose }

    mutating func process(_ data: Data) throws -> AHPProcessedFrame {
        do { return try decode(data) }
        catch let error as ClientError { throw error }
        catch { throw ClientError.invalidResponse }
    }
    private struct Header: Decodable {
        let jsonrpc: String
        let id: RequestID?
        let method: String?
        let error: Fault?
    }
    private struct Fault: Decodable { let code: Int }
    private struct Reply<Result: Decodable>: Decodable { let result: Result }
    private struct Notification: Decodable { let params: AHPActionEnvelope }

    private mutating func decode(_ data: Data) throws -> AHPProcessedFrame {
        guard data.count <= 2 * 1024 * 1024 else { throw ClientError.responseTooLarge }
        let decoder = JSONDecoder()
        let header = try decoder.decode(Header.self, from: data)
        guard header.jsonrpc == "2.0" else { throw ClientError.invalidResponse }
        var events: [ClientEvent] = []
        if let id = header.id {
            guard header.method == nil else { throw ClientError.invalidResponse }
            guard let purpose = pending.removeValue(forKey: id) else {
                events.append(noteUnsupported())
                return AHPProcessedFrame(completedRequestID: nil, events: events)
            }
            if let fault = header.error { throw ClientError.rpc(code: fault.code) }
            switch purpose {
            case .initialize:
                let result = try decoder.decode(Reply<InitializeResult>.self, from: data).result
                guard !initialized, result.protocolVersion == "1.0.0",
                      Set(result.snapshots.map(\.channel)) == subscriptions,
                      result.snapshots.count == subscriptions.count else { throw ClientError.invalidResponse }
                serverSeq = max(serverSeq, result.serverSeq)
                for snapshot in result.snapshots { events += project(snapshot) }
                initialized = true
                events.append(.initialized(protocolVersion: result.protocolVersion))
            case .subscribe(let channel):
                let result = try decoder.decode(Reply<SubscribeResult>.self, from: data).result
                guard initialized, result.snapshot.channel == channel else { throw ClientError.invalidResponse }
                subscriptions.insert(channel)
                events += project(result.snapshot)
            }
            return AHPProcessedFrame(completedRequestID: id, events: events)
        }
        if let fault = header.error { throw ClientError.rpc(code: fault.code) }
        if header.method == "action" {
            guard initialized else { throw ClientError.invalidResponse }
            let envelope = try decoder.decode(Notification.self, from: data).params
            guard envelope.serverSeq > serverSeq else {
                duplicateEventCount += 1
                return AHPProcessedFrame(completedRequestID: nil, events: [])
            }
            serverSeq = envelope.serverSeq
            if envelope.action.isUnsupported { events.append(noteUnsupported()) }
            events.append(.action(envelope))
        } else { events.append(noteUnsupported()) }
        return AHPProcessedFrame(completedRequestID: nil, events: events)
    }
    private mutating func project(_ snapshot: WireSnapshot) -> [ClientEvent] {
        serverSeq = max(serverSeq, snapshot.fromSeq)
        switch snapshot {
        case .chat(let value): return [.snapshot(value)]
        case .session(let value): return [.sessionSnapshot(value)]
        case .root: return [noteUnsupported()]
        }
    }
    private mutating func noteUnsupported() -> ClientEvent {
        unsupportedEventCount += 1
        return .unsupportedEvent(total: unsupportedEventCount)
    }
}

extension WireSnapshot {
    var channel: AHPChannel {
        switch self {
        case .chat(let value): return .chat(value.resource)
        case .session(let value): return .session(value.resource)
        case .root: return .root
        }
    }
    var fromSeq: UInt64 {
        switch self {
        case .chat(let value): return value.fromSeq
        case .session(let value): return value.fromSeq
        case .root: return 0
        }
    }
}
