import SwiftUI

@MainActor
struct ClientCommands: Commands {
    @ObservedObject var model: NativeClientModel
    @Environment(\.openWindow) private var openWindow

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Show Bears") { openWindow(id: "bears-client") }.keyboardShortcut("n")
        }
        CommandMenu("Conversation") {
            Button("Reconnect live chat") { model.reconnect() }
                .disabled(model.selectedConversation == nil || model.authentication != .signedIn || model.composer.activeTurnID != nil)
            Button("Refresh canonical history") { model.refreshHistory() }
                .disabled(model.selectedConversation == nil || model.isLoadingHistory || model.composer.activeTurnID != nil)
            Divider()
            Button("Stop reply") { model.stopReply() }.keyboardShortcut(".", modifiers: .command).disabled(!model.canStop)
        }
    }
}
