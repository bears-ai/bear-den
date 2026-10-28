import Foundation
import BearsClient

@MainActor
extension NativeClientModel {
    func selectConversation(_ conversation: NativeConversation) {
        guard selectedConversation?.chatURI != conversation.chatURI else { return }
        stopCurrentChat(saveDraft: true)
        selectedConversation = conversation
        composer = savedComposers.removeValue(forKey: conversation.chatURI) ?? ComposerState()
        transcript = TranscriptReducer(chat: conversation.chatURI)
        issue = nil
        loadHistory()
        connectSelectedChat()
    }
    func reconnect() {
        guard let selected = selectedConversation, authentication == .signedIn else { return }
        detachSocket()
        transcript = TranscriptReducer(chat: selected.chatURI)
        hasSnapshot = false
        issue = nil
        loadHistory()
        connectSelectedChat()
    }
    /// Also used when the main window closes. Keep visible data, but never leave inference running unseen.
    func disconnectChat() { detachSocket() }
    func detachSocket() {
        chatGeneration = OperationGeneration()
        streamTask?.cancel(); streamTask = nil; sendTask?.cancel(); sendTask = nil
        acceptanceTask?.cancel(); acceptanceTask = nil
        historyTask?.cancel(); historyTask = nil; stopTask?.cancel(); stopTask = nil
        composer.disconnected(); transcript?.apply(.disconnected)
        let previous = socket
        let barrier = disconnectBarrier
        socket = nil
        disconnectBarrier = Task { await barrier?.value; await previous?.disconnect() }
        connectionID = nil; connection = .disconnected; isStopping = false; isLoadingHistory = false
    }
    func connectSelectedChat() {
        guard let selected = selectedConversation, let configuration, let token else { return }
        let generation = chatGeneration
        let newSocket = dependencies.makeSocket(configuration)
        let barrier = disconnectBarrier
        socket = newSocket
        connection = .connecting
        streamTask = Task {
            await barrier?.value
            guard chatGeneration == generation, !Task.isCancelled else { await newSocket.disconnect(); return }
            do {
                let events = try await newSocket.connect(token: token,
                    subscriptions: [.session(selected.sessionURI), .chat(selected.chatURI)])
                guard chatGeneration == generation, !Task.isCancelled else { await newSocket.disconnect(); return }
                for try await event in events {
                    guard chatGeneration == generation, !Task.isCancelled else { await newSocket.disconnect(); return }
                    receive(event, expectedClientID: configuration.clientID)
                }
                guard chatGeneration == generation, !Task.isCancelled else { return }
                connection = .disconnected; connectionID = nil; composer.disconnected(); isStopping = false
                transcript?.apply(.disconnected)
            } catch {
                guard chatGeneration == generation, !Task.isCancelled else { return }
                connection = .disconnected; connectionID = nil; composer.disconnected(); isStopping = false
                transcript?.apply(.disconnected)
                report(error)
            }
        }
    }
    func receive(_ event: ClientEvent, expectedClientID: String) {
        transcript?.apply(event)
        switch event {
        case .connected(let id): connectionID = id
        case .initialized: connection = hasSnapshot ? .ready : .disconnected
        case .snapshot(let snapshot):
            guard snapshot.resource == selectedConversation?.chatURI else { return }
            hasSnapshot = true
            selectedConversation?.title = snapshot.summary.title
            selectedConversation?.canSend = snapshot.summary.canSend
            composer.reconcileActiveTurn(snapshot.activeTurn?.id)
        case .action(let envelope):
            guard envelope.channel == selectedConversation.map({ .chat($0.chatURI) }), let connectionID else { return }
            let wasPending = composer.hasPendingSend
            composer.observe(envelope, connectionID: connectionID, expectedClientID: expectedClientID)
            if wasPending && !composer.hasPendingSend && !composer.wasRejected { issue = nil }
            if envelope.rejectionReason != nil {
                isStopping = false
                issue = "Den did not accept the action. The draft has been retained. Refresh history before retrying an uncertain send."
            } else {
                switch envelope.action {
                case .turnComplete, .cancelled: isStopping = false
                case .error:
                    isStopping = false
                    issue = "The reply did not complete. Partial text may be saved; refresh canonical history before sending again."
                default: break
                }
            }
        case .disconnected:
            connection = .disconnected; connectionID = nil; composer.disconnected(); isStopping = false
        case .sessionSnapshot, .unsupportedEvent: break
        }
    }
    func send() {
        guard canSend, let selected = selectedConversation, let socket, let configuration,
              let connection = connectionID, let submission = composer.begin() else { return }
        let generation = chatGeneration
        issue = nil
        acceptanceTask?.cancel()
        acceptanceTask = Task {
            do { try await Task.sleep(nanoseconds: 30_000_000_000) } catch { return }
            guard chatGeneration == generation, composer.pendingTurnID == submission.turnID else { return }
            composer.transmissionFailed()
            issue = "Den has not acknowledged this message. Its delivery is uncertain; the draft is retained."
        }
        sendTask = Task {
            guard chatGeneration == generation, !Task.isCancelled else { return }
            do {
                let receipt = try await socket.send(text: submission.text, to: selected.chatURI,
                                                    turnID: submission.turnUUID, at: Date())
                guard chatGeneration == generation, !Task.isCancelled else { return }
                guard receipt.connectionID == connection else { composer.transmissionFailed(); return }
                composer.record(receipt, expectedClientID: configuration.clientID)
                if !composer.hasPendingSend && !composer.wasRejected { issue = nil }
            } catch {
                guard chatGeneration == generation, !Task.isCancelled else { return }
                composer.transmissionFailed()
                report(error)
            }
        }
    }
    func stopReply() {
        guard canStop, let turn = composer.activeTurnID, let socket, let chat = selectedConversation?.chatURI else { return }
        let generation = chatGeneration
        isStopping = true
        stopTask = Task {
            guard chatGeneration == generation, !Task.isCancelled else { return }
            do { _ = try await socket.cancel(turnID: turn, in: chat) }
            catch {
                guard chatGeneration == generation, !Task.isCancelled else { return }
                isStopping = false
                report(error)
            }
        }
    }
    func refreshHistory() {
        guard selectedConversation != nil else { return }
        detachSocket()
        hasSnapshot = false
        history = []; historyBefore = nil; historyHasMore = false
        issue = nil
        loadHistory()
    }
    func loadOlderHistory() {
        guard !hasSnapshot, historyHasMore, let before = historyBefore else { return }
        loadHistory(before: before)
    }
    func loadHistory(before: Int64? = nil) {
        guard !isLoadingHistory, let bearID = selectedBearID, let selected = selectedConversation, let api, let token else { return }
        let generation = chatGeneration
        isLoadingHistory = true
        historyTask = Task {
            do {
                let page = try await api.history(bearID: bearID, conversationID: selected.id, before: before, limit: 50, token: token)
                guard chatGeneration == generation, !Task.isCancelled else { return }
                let combined = before == nil ? page.messages : page.messages + history
                var seen: Set<MessageID> = []
                history = combined.filter { seen.insert($0.id).inserted }.sorted { $0.sequenceNo < $1.sequenceNo }
                historyBefore = page.beforeSequence
                historyHasMore = page.hasMore && historyBefore != nil
                isLoadingHistory = false
            } catch {
                guard chatGeneration == generation, !Task.isCancelled else { return }
                isLoadingHistory = false
                report(error)
            }
        }
    }
}
