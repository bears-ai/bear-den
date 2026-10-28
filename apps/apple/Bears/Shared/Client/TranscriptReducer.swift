import Foundation

/// A disposable UI projection, not another canonical database. Feed one connection's event stream in order.
/// Canonical snapshots replace the projection, including interrupted or optimistic-looking live data.
public struct TranscriptReducer: Sendable {
    public let chat: ChatURI
    public private(set) var turns: [ChatTurn] = []
    public private(set) var connectionID: ConnectionID?
    public private(set) var lastServerSeq: UInt64 = 0
    public private(set) var needsSnapshot = true
    public private(set) var isConnected = false
    public private(set) var unsupportedEventCount: UInt64 = 0
    public private(set) var ignoredEventCount: UInt64 = 0
    public private(set) var lastRejectionReason: String?

    public init(chat: ChatURI) { self.chat = chat }

    public mutating func apply(_ event: ClientEvent) {
        switch event {
        case .connected(let id):
            connectionID = id; isConnected = true; needsSnapshot = true
            lastServerSeq = 0; unsupportedEventCount = 0; lastRejectionReason = nil
        case .snapshot(let snapshot): reconcile(snapshot)
        case .action(let envelope): apply(envelope)
        case .unsupportedEvent(let total): unsupportedEventCount = max(unsupportedEventCount, total)
        case .disconnected:
            isConnected = false; needsSnapshot = true
        case .initialized, .sessionSnapshot: break
        }
    }

    private mutating func reconcile(_ snapshot: ChatSnapshot) {
        guard snapshot.resource == chat, snapshot.fromSeq >= lastServerSeq else { ignoredEventCount += 1; return }
        // A live turn supersedes the same durable ID; never append both versions.
        var replacement: [ChatTurn] = []
        for turn in snapshot.turns + (snapshot.activeTurn.map { [$0] } ?? []) {
            if let index = replacement.firstIndex(where: { $0.id == turn.id }) { replacement[index] = turn }
            else { replacement.append(turn) }
        }
        turns = replacement
        lastServerSeq = snapshot.fromSeq
        needsSnapshot = false
        lastRejectionReason = nil
    }

    private mutating func apply(_ envelope: AHPActionEnvelope) {
        guard envelope.channel == .chat(chat) else { return }
        guard !needsSnapshot, envelope.serverSeq > lastServerSeq else { ignoredEventCount += 1; return }
        lastServerSeq = envelope.serverSeq
        // Rejections echo the submitted action; they must not create a phantom accepted turn.
        if let reason = envelope.rejectionReason { lastRejectionReason = reason; return }
        switch envelope.action {
        case .turnStarted(let value):
            guard !turns.contains(where: { $0.id == value.turnID }) else { ignoredEventCount += 1; return }
            turns.append(ChatTurn(id: value.turnID, startedAt: value.startedAt, message: value.message))
        case .unsupported:
            break
        default:
            guard let turnID = envelope.action.turnID,
                  let index = turns.firstIndex(where: { $0.id == turnID }), turns[index].state == .streaming else {
                ignoredEventCount += 1; return
            }
            updateTurn(at: index, action: envelope.action)
        }
    }
    private mutating func updateTurn(at index: Int, action: AHPAction) {
        switch action {
        case .responsePart(let value):
            if case .markdown(let part) = value.part,
               turns[index].responseParts.contains(where: {
                   if case .markdown(let existing) = $0 { return existing.id == part.id }; return false
               }) { ignoredEventCount += 1; return }
            turns[index].responseParts.append(value.part)
        case .delta(let value):
            guard let partIndex = turns[index].responseParts.firstIndex(where: {
                if case .markdown(let part) = $0 { return part.id == value.partID }; return false
            }), case .markdown(var part) = turns[index].responseParts[partIndex] else {
                ignoredEventCount += 1; return
            }
            part.content += value.content
            turns[index].responseParts[partIndex] = .markdown(part)
        case .turnComplete(let end): finish(index, state: .complete, duration: end.duration)
        case .cancelled(let end): finish(index, state: .cancelled, duration: end.duration)
        case .error(let failure):
            turns[index].responseParts.append(failure.part)
            finish(index, state: .error, duration: failure.duration)
        case .turnStarted, .unsupported: break
        }
    }
    private mutating func finish(_ index: Int, state: TurnState, duration: UInt64) {
        turns[index].state = state
        turns[index].duration = duration
    }
}
