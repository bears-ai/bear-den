import Foundation
import Combine
import BearsClient

/// One instance is owned by the App, not by each window or view.
@MainActor
final class NativeClientModel: ObservableObject {
    @Published var serverURLText: String
    @Published var publicClientIDText: String
    @Published var configuration: ClientConfiguration?
    @Published var authentication: ClientAuthentication = .signedOut
    @Published var user: UserInfo?
    @Published var issue: String?
    @Published var failedSignOuts: [ClientConfiguration] = []
    var failedSignOut: ClientConfiguration? { failedSignOuts.first }
    @Published var bears: [BearSummary] = []
    @Published var selectedBearID: BearID?
    @Published var hats: [HatSummary] = []
    @Published var conversations: [ConversationSummary] = []
    @Published var selectedConversation: NativeConversation?
    @Published var newHatID: HatID?
    @Published var pendingCreation: CreateConversation?
    @Published var isLoadingMembership = false
    @Published var isLoadingInventory = false
    @Published var isCreating = false
    @Published var connection: ChatConnection = .disconnected
    @Published var transcript: TranscriptReducer?
    @Published var hasSnapshot = false
    @Published var history: [HistoryMessage] = []
    @Published var historyBefore: Int64?
    @Published var historyHasMore = false
    @Published var isLoadingHistory = false
    @Published var composer = ComposerState()
    @Published var isStopping = false

    let dependencies: ClientDependencies
    var token: OAuthToken?
    var api: (any NativeAPI)?
    var socket: (any NativeSocket)?
    var sessionGeneration = OperationGeneration()
    var bearGeneration = OperationGeneration()
    var chatGeneration = OperationGeneration()
    var connectionID: ConnectionID?
    var savedComposers: [ChatURI: ComposerState] = [:]
    var reauthorizationDrafts: ReauthorizationDrafts?
    var started = false
    var authTask: Task<Void, Never>?
    var expiryTask: Task<Void, Never>?
    var catalogTask: Task<Void, Never>?
    var inventoryTask: Task<Void, Never>?
    var creationTask: Task<Void, Never>?
    var streamTask: Task<Void, Never>?
    var historyTask: Task<Void, Never>?
    var sendTask: Task<Void, Never>?
    var acceptanceTask: Task<Void, Never>?
    var stopTask: Task<Void, Never>?
    var disconnectBarrier: Task<Void, Never>?

    init(dependencies: ClientDependencies) {
        self.dependencies = dependencies
        serverURLText = dependencies.preferences.serverURL
        publicClientIDText = dependencies.preferences.publicClientID
    }
    deinit {
        authTask?.cancel(); expiryTask?.cancel(); catalogTask?.cancel(); inventoryTask?.cancel()
        creationTask?.cancel(); streamTask?.cancel(); historyTask?.cancel(); sendTask?.cancel(); stopTask?.cancel(); acceptanceTask?.cancel()
        let previous = socket
        Task { await previous?.disconnect() }
    }
    static func live() -> NativeClientModel { NativeClientModel(dependencies: .live()) }

    var selectedBear: BearSummary? { bears.first { $0.id == selectedBearID } }
    var accountName: String { user?.name ?? user?.preferredUsername ?? "Signed in" }
    var canSend: Bool {
        authentication == .signedIn && user != nil && connection == .ready && !isStopping &&
        selectedConversation?.canSend == true && composer.canSubmit && hasSnapshot
    }
    var canStop: Bool {
        connection == .ready && composer.activeTurnID != nil && !isStopping
    }
    func start() {
        guard !started else { return }
        started = true
        if !serverURLText.isEmpty || !publicClientIDText.isEmpty { configure() }
    }
    func configure() {
        do {
            guard let url = URL(string: serverURLText.trimmingCharacters(in: .whitespacesAndNewlines)) else {
                throw ClientError.invalidConfiguration
            }
            let configuration = try ClientConfiguration(baseURL: url,
                clientID: publicClientIDText.trimmingCharacters(in: .whitespacesAndNewlines))
            resetRuntime()
            self.configuration = configuration
            serverURLText = configuration.baseURL.absoluteString
            publicClientIDText = configuration.clientID
            dependencies.preferences.save(configuration)
            api = dependencies.makeAPI(configuration)
            issue = nil
            if !failedSignOuts.contains(configuration), let saved = try dependencies.vault.load(for: configuration) {
                if saved.isExpired() { authentication = .needsReauthorization }
                else { acceptToken(saved) }
            }
        } catch { issue = Self.safeMessage(error) }
    }
    func resetRuntime() {
        sessionGeneration = OperationGeneration(); bearGeneration = OperationGeneration()
        authTask?.cancel(); authTask = nil; dependencies.browser.cancel()
        expiryTask?.cancel(); expiryTask = nil; catalogTask?.cancel(); catalogTask = nil
        inventoryTask?.cancel(); inventoryTask = nil; creationTask?.cancel(); creationTask = nil
        stopCurrentChat(saveDraft: false)
        savedComposers.removeAll(); reauthorizationDrafts = nil
        token = nil; api = nil; user = nil; authentication = .signedOut
        bears = []; selectedBearID = nil; hats = []; conversations = []; newHatID = nil
        pendingCreation = nil; isCreating = false; isLoadingInventory = false; isLoadingMembership = false
    }
    func stopCurrentChat(saveDraft: Bool) {
        detachSocket()
        if saveDraft, let selected = selectedConversation { savedComposers[selected.chatURI] = composer }
        selectedConversation = nil; transcript = nil; hasSnapshot = false
        history = []; historyBefore = nil; historyHasMore = false; isLoadingHistory = false
        composer = ComposerState()
    }
    func report(_ error: Error) {
        if let client = error as? ClientError {
            switch client {
            case .expiredToken, .http(status: 401, code: _):
                requireReauthorization()
            default: break
            }
        }
        issue = Self.safeMessage(error)
    }
    static func safeMessage(_ error: Error) -> String {
        if let error = error as? ClientError { return error.errorDescription ?? "The request failed." }
        if let error = error as? ClientPlatformError { return error.errorDescription ?? "The platform operation failed." }
        if error is CancellationError { return "The operation was cancelled." }
        return "The operation could not be completed. Refresh canonical history before retrying a send."
    }
}
