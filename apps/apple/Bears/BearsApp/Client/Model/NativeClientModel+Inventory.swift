import Foundation
import BearsClient

@MainActor
extension NativeClientModel {
    func selectBear(_ id: BearID?) {
        guard id != selectedBearID else { return }
        bearGeneration = OperationGeneration()
        inventoryTask?.cancel(); creationTask?.cancel()
        stopCurrentChat(saveDraft: true)
        selectedBearID = id
        hats = []; conversations = []; newHatID = nil; pendingCreation = nil; isCreating = false
        isLoadingInventory = false
        issue = nil
        if id != nil { refreshInventory() }
    }
    func refreshInventory() {
        guard let bearID = selectedBearID, let token, let api else { return }
        inventoryTask?.cancel()
        let session = sessionGeneration
        let generation = bearGeneration
        isLoadingInventory = true
        inventoryTask = Task {
            do {
                let inventory = try await api.conversations(bearID: bearID, token: token)
                guard sessionGeneration == session, bearGeneration == generation, !Task.isCancelled else { return }
                hats = inventory.hats; conversations = inventory.conversations
                if newHatID == nil || !hats.contains(where: { $0.id == newHatID }) { newHatID = nil }
                if let selected = selectedConversation {
                    if let updated = conversations.first(where: { $0.id == selected.id }) { selectedConversation = NativeConversation(updated) }
                    else { stopCurrentChat(saveDraft: true) }
                }
                isLoadingInventory = false
            } catch {
                guard sessionGeneration == session, bearGeneration == generation, !Task.isCancelled else { return }
                isLoadingInventory = false
                report(error)
            }
        }
    }
    func createConversation() {
        guard !isCreating, let bearID = selectedBearID, let api, let token else { return }
        let input: CreateConversation
        if let pendingCreation { input = pendingCreation }
        else {
            guard let hatID = newHatID, hats.contains(where: { $0.id == hatID }) else { return }
            input = CreateConversation(bearID: bearID, hatID: hatID)
            pendingCreation = input
        }
        guard input.bearID == bearID else { return }
        let session = sessionGeneration
        let generation = bearGeneration
        isCreating = true; issue = nil
        creationTask = Task {
            guard sessionGeneration == session, bearGeneration == generation, !Task.isCancelled else { return }
            do {
                let created = try await api.createConversation(input, token: token)
                guard sessionGeneration == session, bearGeneration == generation, !Task.isCancelled else { return }
                isCreating = false; pendingCreation = nil
                selectConversation(NativeConversation(created))
                refreshInventory()
            } catch {
                guard sessionGeneration == session, bearGeneration == generation, !Task.isCancelled else { return }
                isCreating = false
                // Keep the exact bear/hat/request tuple for an explicit retry after an uncertain result.
                report(error)
            }
        }
    }
    func abandonCreation() {
        guard !isCreating else { return }
        pendingCreation = nil
        issue = "Creation request dismissed. Refresh the conversation list before creating another; the previous request may have succeeded."
    }
    func hatName(_ id: HatID?) -> String {
        guard let id else { return "No active hat" }
        return hats.first(where: { $0.id == id })?.name ?? "Hat unavailable"
    }
}
