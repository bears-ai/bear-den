import Foundation
import BearsClient

/// Ephemeral composer state only. Transcript acceptance is always driven by Den, never by send() returning.
struct ComposerState: Sendable {
    struct Submission: Sendable {
        let turnUUID: UUID
        let text: String
        var turnID: TurnID { TurnID(turnUUID.uuidString.lowercased()) }
    }
    private struct Pending: Sendable {
        let submission: Submission
        var receipt: DispatchReceipt?
        var observedAcceptance: Acceptance?
    }
    private struct Acceptance: Sendable {
        let originClientID: String
        let clientSeq: UInt64
        let connectionID: ConnectionID
        let isRejection: Bool
    }
    var draft = ""
    private var pending: Pending?
    private(set) var activeTurnID: TurnID?
    private(set) var deliveryUncertain = false
    private(set) var wasRejected = false
    var hasPendingSend: Bool { pending != nil }
    var pendingTurnID: TurnID? { pending?.submission.turnID }
    var canSubmit: Bool {
        pending == nil && activeTurnID == nil && !deliveryUncertain &&
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && draft.utf8.count <= 32 * 1024 && !draft.contains("\0")
    }
    var status: String? {
        if deliveryUncertain { return "Delivery is uncertain. Your draft is retained. Refresh history before deciding whether to send again." }
        if pending != nil { return "Waiting for Den to accept the message. Your draft is retained." }
        if activeTurnID != nil { return "Bear is replying…" }
        if wasRejected { return "Den did not accept the message. Your draft is retained." }
        return nil
    }
    mutating func begin() -> Submission? {
        guard canSubmit else { return nil }
        let submission = Submission(turnUUID: UUID(), text: draft)
        pending = Pending(submission: submission)
        wasRejected = false
        return submission
    }
    mutating func record(_ receipt: DispatchReceipt, expectedClientID: String) {
        guard pending?.submission.turnID == receipt.turnID else { return }
        pending?.receipt = receipt
        acceptIfCorrelated(expectedClientID: expectedClientID)
    }
    mutating func observe(_ envelope: AHPActionEnvelope, connectionID: ConnectionID, expectedClientID: String) {
        if let pending, envelope.action.turnID == pending.submission.turnID, envelope.rejectionReason != nil {
            // A rejection without the matching origin is not an acknowledgment of our command.
            if let receipt = pending.receipt, let origin = envelope.origin,
               receipt.connectionID == connectionID, origin.clientID == expectedClientID, origin.clientSeq == receipt.clientSeq {
                self.pending = nil; deliveryUncertain = false; wasRejected = true
            } else if pending.receipt == nil, let origin = envelope.origin, origin.clientID == expectedClientID {
                // Preserve an early rejection until send() supplies the correlation receipt.
                self.pending?.observedAcceptance = Acceptance(originClientID: origin.clientID, clientSeq: origin.clientSeq,
                                                            connectionID: connectionID, isRejection: true)
            }
            return
        }
        guard envelope.rejectionReason == nil else { return }
        if case .turnStarted(let start) = envelope.action {
            activeTurnID = start.turnID
            if pending?.submission.turnID == start.turnID, let origin = envelope.origin {
                pending?.observedAcceptance = Acceptance(originClientID: origin.clientID, clientSeq: origin.clientSeq,
                                                                         connectionID: connectionID, isRejection: false)
                acceptIfCorrelated(expectedClientID: expectedClientID)
            }
        }
        switch envelope.action {
        case .turnComplete(let end), .cancelled(let end): if activeTurnID == end.turnID { activeTurnID = nil }
        case .error(let end): if activeTurnID == end.turnID { activeTurnID = nil }
        default: break
        }
    }
    private mutating func acceptIfCorrelated(expectedClientID: String) {
        guard let pending, let receipt = pending.receipt, let observed = pending.observedAcceptance,
              observed.connectionID == receipt.connectionID, observed.originClientID == expectedClientID,
              observed.clientSeq == receipt.clientSeq else { return }
        wasRejected = observed.isRejection
        if !observed.isRejection, draft == pending.submission.text { draft = "" }
        self.pending = nil
        deliveryUncertain = false
    }
    mutating func disconnected() {
        if pending != nil { deliveryUncertain = true }
        activeTurnID = nil
    }
    mutating func transmissionFailed() { if pending != nil { deliveryUncertain = true } }
    mutating func reconcileActiveTurn(_ turnID: TurnID?) { activeTurnID = turnID }
    /// User-only escape hatch after reviewing canonical history. This never sends anything.
    mutating func resolveUncertainDelivery() {
        pending = nil; deliveryUncertain = false; wasRejected = false
    }
}
