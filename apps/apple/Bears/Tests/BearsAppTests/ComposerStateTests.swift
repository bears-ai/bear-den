import Foundation
import XCTest
@testable import BearsClient
@testable import BearsApp

final class ComposerStateTests: XCTestCase {
    func testDraftClearsOnlyAfterMatchingServerAcceptanceAndReceipt() throws {
        var composer = ComposerState()
        composer.draft = "Draft"
        let send = try XCTUnwrap(composer.begin())
        composer.record(DispatchReceipt(connectionID: NativeFixtures.connection, clientSeq: 7, turnID: send.turnID), expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Draft")
        composer.observe(try NativeFixtures.action(turn: send.turnID, seq: 6), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Draft", "A different clientSeq is not acceptance")
        composer.observe(try NativeFixtures.action(turn: send.turnID, seq: 7, clientID: "other-client"), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Draft")
        composer.observe(try NativeFixtures.action(turn: send.turnID, seq: 7), connectionID: ConnectionID(UUID()), expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Draft", "An old/new connection cannot acknowledge this send")
        composer.observe(try NativeFixtures.action(turn: send.turnID, seq: 7), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "")
        XCTAssertFalse(composer.hasPendingSend)
        XCTAssertEqual(composer.activeTurnID, send.turnID)
    }
    func testAcceptanceCanArriveBeforeSendReturnsWithoutLosingNewDraftEdits() throws {
        var composer = ComposerState()
        composer.draft = "Draft"
        let send = try XCTUnwrap(composer.begin())
        composer.observe(try NativeFixtures.action(turn: send.turnID), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Draft")
        composer.draft = "Next message being edited"
        composer.record(DispatchReceipt(connectionID: NativeFixtures.connection, clientSeq: 1, turnID: send.turnID), expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Next message being edited")
        XCTAssertFalse(composer.hasPendingSend)
    }
    func testEarlyRejectionRetainsDraftAfterReceipt() throws {
        var composer = ComposerState()
        composer.draft = "Draft"
        let send = try XCTUnwrap(composer.begin())
        composer.observe(try NativeFixtures.action(turn: send.turnID, rejection: true), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        composer.record(DispatchReceipt(connectionID: NativeFixtures.connection, clientSeq: 1, turnID: send.turnID), expectedClientID: "apple-test")
        XCTAssertEqual(composer.draft, "Draft")
        XCTAssertTrue(composer.wasRejected)
        XCTAssertFalse(composer.hasPendingSend)
        XCTAssertNil(composer.activeTurnID)
    }
    func testDisconnectKeepsUncertainDraftAndNeverResubmits() throws {
        var composer = ComposerState()
        composer.draft = "Draft"
        _ = try XCTUnwrap(composer.begin())
        composer.disconnected()
        XCTAssertEqual(composer.draft, "Draft")
        XCTAssertTrue(composer.deliveryUncertain)
        XCTAssertFalse(composer.canSubmit)
        XCTAssertNil(composer.begin())
        composer.resolveUncertainDelivery()
        XCTAssertEqual(composer.draft, "Draft")
        XCTAssertTrue(composer.canSubmit, "Only the explicit user resolution releases the pending draft")
    }
    func testRejectedCancellationDoesNotPretendReplyStopped() throws {
        var composer = ComposerState()
        let turn = TurnID(NativeFixtures.turn.uuidString.lowercased())
        composer.reconcileActiveTurn(turn)
        composer.observe(try NativeFixtures.action(turn: turn, rejection: true, type: "chat/turnCancelled"), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        XCTAssertEqual(composer.activeTurnID, turn)
        composer.observe(try NativeFixtures.action(turn: turn, type: "chat/turnCancelled"), connectionID: NativeFixtures.connection, expectedClientID: "apple-test")
        XCTAssertNil(composer.activeTurnID)
    }
    func testBlankOversizedAndNULDraftsAreNotSendable() {
        for text in [" \n", String(repeating: "x", count: 32 * 1024 + 1), "hello\0there"] {
            var composer = ComposerState()
            composer.draft = text
            XCTAssertFalse(composer.canSubmit)
            XCTAssertNil(composer.begin())
        }
    }
}
