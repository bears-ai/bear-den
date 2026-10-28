import Foundation
import XCTest
@testable import BearsClient
@testable import BearsApp

final class NativeClientModelTests: XCTestCase {
    @MainActor
    func testStaleInventoryCannotOverwriteNewBearSelection() async throws {
        let api = TestNativeAPI()
        await api.suspendInventory()
        let model = makeClientModel(api: api)
        model.token = NativeFixtures.token; model.authentication = .signedIn
        model.bears = try NativeFixtures.bears()
        model.selectBear(NativeFixtures.bearA)
        let oldTask = model.inventoryTask
        await waitUntil { await api.hasInventory(NativeFixtures.bearA) }
        model.selectBear(NativeFixtures.bearB)
        await waitUntil { await api.hasInventory(NativeFixtures.bearB) }
        try await api.finishInventory(NativeFixtures.bearB, title: "B hats")
        await model.inventoryTask?.value
        try await api.finishInventory(NativeFixtures.bearA, title: "Stale A hats")
        await oldTask?.value
        XCTAssertEqual(model.selectedBearID, NativeFixtures.bearB)
        XCTAssertEqual(model.hats.first?.name, "B hats")
        model.signOut()
    }
    @MainActor
    func testCreationRetryPreservesExplicitHatAndRequestUUID() async throws {
        let api = TestNativeAPI()
        let model = makeClientModel(api: api)
        model.token = NativeFixtures.token; model.authentication = .signedIn
        model.bears = try NativeFixtures.bears()
        model.selectBear(NativeFixtures.bearA)
        await model.inventoryTask?.value
        XCTAssertNil(model.newHatID, "The first hat must not be chosen implicitly")
        model.newHatID = NativeFixtures.hat
        model.createConversation()
        await model.creationTask?.value
        let pending = try XCTUnwrap(model.pendingCreation)
        XCTAssertFalse(model.isCreating)
        model.createConversation()
        await model.creationTask?.value
        let requests = await api.creationRequests
        XCTAssertEqual(requests, [pending, pending])
        XCTAssertNil(model.pendingCreation)
        XCTAssertEqual(model.selectedConversation?.hatID, NativeFixtures.hat)
        model.signOut()
    }
    @MainActor
    func testSwitchingChatsDisconnectsOldSocketAndRetainsDraftWithoutOldEvents() async throws {
        let first = TestNativeSocket()
        let second = TestNativeSocket()
        let model = makeClientModel(sockets: [first, second])
        model.token = NativeFixtures.token; model.authentication = .signedIn
        model.selectedBearID = NativeFixtures.bearA
        model.selectConversation(try NativeFixtures.conversation())
        await waitUntil { model.connection == .ready }
        model.composer.draft = "Unsent A draft"
        model.selectConversation(try NativeFixtures.conversation(NativeFixtures.chatB))
        await waitUntil { model.connection == .ready }
        let disconnected = await first.disconnects
        XCTAssertGreaterThanOrEqual(disconnected, 1)
        await first.emit(.disconnected)
        await Task.yield()
        XCTAssertEqual(model.selectedConversation?.chatURI, NativeFixtures.chatB)
        XCTAssertEqual(model.transcript?.chat, NativeFixtures.chatB)
        XCTAssertEqual(model.connection, .ready)
        XCTAssertTrue(model.composer.draft.isEmpty)
        XCTAssertEqual(model.savedComposers[NativeFixtures.chatA]?.draft, "Unsent A draft")
        model.signOut()
        await model.disconnectBarrier?.value
        let secondDisconnects = await second.disconnectCount()
        XCTAssertGreaterThanOrEqual(secondDisconnects, 1)
    }
    @MainActor
    func testLateOldConnectionCannotReplaceNewChatState() async throws {
        let first = TestNativeSocket()
        await first.suspendConnect()
        let second = TestNativeSocket()
        let model = makeClientModel(sockets: [first, second])
        model.token = NativeFixtures.token; model.authentication = .signedIn
        model.selectedBearID = NativeFixtures.bearA
        model.selectConversation(try NativeFixtures.conversation())
        let oldTask = model.streamTask
        await waitUntil { await first.isConnectPending() }
        model.selectConversation(try NativeFixtures.conversation(NativeFixtures.chatB))
        await waitUntil { model.connection == .ready }
        let currentID = model.connectionID
        await first.finishConnect()
        await oldTask?.value
        XCTAssertEqual(model.selectedConversation?.chatURI, NativeFixtures.chatB)
        XCTAssertEqual(model.connectionID, currentID)
        XCTAssertEqual(model.connection, .ready)
        XCTAssertEqual(model.transcript?.chat, NativeFixtures.chatB)
        model.signOut()
    }
    @MainActor
    func testFailedKeychainSaveDoesNotClaimPersistentLogin() async throws {
        let vault = TestVault()
        vault.failSave = true
        let model = makeClientModel(vault: vault)
        model.signIn()
        await model.authTask?.value
        XCTAssertEqual(model.authentication, .signedOut)
        XCTAssertNil(model.token)
        XCTAssertTrue(vault.saved.isEmpty)
        XCTAssertNotNil(model.issue)
        model.signOut()
    }
    @MainActor
    func testImmediateSignOutPreventsQueuedCreationFromStarting() async throws {
        let api = TestNativeAPI()
        let model = makeClientModel(api: api)
        model.token = NativeFixtures.token; model.authentication = .signedIn
        model.selectedBearID = NativeFixtures.bearA
        model.hats = try NativeFixtures.inventory().hats
        model.newHatID = NativeFixtures.hat
        model.createConversation()
        let queued = model.creationTask
        model.signOut()
        await queued?.value
        let requests = await api.creationRequests
        XCTAssertTrue(requests.isEmpty)
    }
    @MainActor
    func testLateTokenExchangeAfterSignOutCannotWriteKeychainOrRestoreLogin() async throws {
        let api = TestNativeAPI()
        await api.suspendExchange()
        let vault = TestVault()
        let model = makeClientModel(api: api, vault: vault)
        model.signIn()
        let oldTask = model.authTask
        await waitUntil { await api.hasExchange() }
        model.signOut()
        await api.finishExchange()
        await oldTask?.value
        XCTAssertEqual(model.authentication, .signedOut)
        XCTAssertNil(model.token)
        XCTAssertTrue(vault.saved.isEmpty)
        XCTAssertTrue(model.bears.isEmpty)
    }
    @MainActor
    func testSignOutClearsRuntimeEvenWhenKeychainRemovalFailsAndCanBeRetried() async throws {
        let vault = TestVault()
        vault.token = NativeFixtures.token
        let model = makeClientModel(vault: vault)
        vault.failRemoval = true
        model.composer.draft = "Private draft"
        model.signOut()
        XCTAssertEqual(model.authentication, .signedOut)
        XCTAssertNil(model.token)
        XCTAssertNil(model.user)
        XCTAssertNil(model.selectedConversation)
        XCTAssertTrue(model.bears.isEmpty)
        XCTAssertTrue(model.composer.draft.isEmpty)
        XCTAssertNotNil(model.failedSignOut)
        XCTAssertNotNil(model.issue)
        XCTAssertFalse(model.issue?.contains("test-only-sensitive-token") ?? true)
        model.configure()
        XCTAssertEqual(model.authentication, .signedOut, "A failed deletion must not silently restore that credential")
        vault.failRemoval = false
        model.retryKeychainRemoval()
        XCTAssertNil(model.failedSignOut)
        XCTAssertNil(vault.token)
    }
    @MainActor
    func testExpiryRetainsPendingDraftOnlyForTheSameTrustedAccount() async throws {
        let model = makeClientModel()
        model.token = NativeFixtures.token; model.authentication = .signedIn
        model.user = try NativeFixtures.profile()
        model.selectedBearID = NativeFixtures.bearA
        model.selectedConversation = try NativeFixtures.conversation()
        model.composer.draft = "Unacknowledged private draft"
        _ = model.composer.begin()
        model.requireReauthorization()
        XCTAssertEqual(model.authentication, .needsReauthorization)
        XCTAssertNil(model.token)
        XCTAssertNotNil(model.reauthorizationDrafts)
        model.signIn()
        await model.authTask?.value
        await model.catalogTask?.value
        let recovered = try XCTUnwrap(model.savedComposers[NativeFixtures.chatA])
        XCTAssertEqual(recovered.draft, "Unacknowledged private draft")
        XCTAssertTrue(recovered.deliveryUncertain)
        XCTAssertFalse(recovered.canSubmit)
        model.savedComposers.removeAll()
        model.reauthorizationDrafts = ReauthorizationDrafts(configuration: try NativeFixtures.configuration(),
            subject: SubjectID("someone-else"), composers: [NativeFixtures.chatA: recovered])
        model.restoreReauthorizationDrafts(for: SubjectID("1"))
        XCTAssertTrue(model.savedComposers.isEmpty, "A different account must never receive the retained draft")
        model.signOut()
        XCTAssertNil(model.reauthorizationDrafts)
    }
    @MainActor
    func testMultipleFailedKeychainRemovalsRemainVisibleUntilAllAreRetried() async throws {
        let vault = TestVault()
        vault.failRemoval = true
        let model = makeClientModel(vault: vault)
        model.signOut()
        model.serverURLText = "https://second.example/den"
        model.configure()
        model.signOut()
        XCTAssertEqual(model.failedSignOuts.count, 2)
        vault.failRemoval = false
        model.retryKeychainRemoval()
        XCTAssertTrue(model.failedSignOuts.isEmpty)
    }
    @MainActor
    func testExpiredSavedTokenRequiresReauthorizationWithoutNetworkRestore() async throws {
        let vault = TestVault()
        vault.token = OAuthToken(accessToken: NativeFixtures.token.accessToken, expiresAt: .distantPast, scope: "")
        let model = makeClientModel(vault: vault)
        XCTAssertEqual(model.authentication, .needsReauthorization)
        XCTAssertNil(model.token)
        XCTAssertNil(model.catalogTask)
        model.signOut()
    }
}

private extension TestNativeSocket {
    func disconnectCount() -> Int { disconnects }
}
