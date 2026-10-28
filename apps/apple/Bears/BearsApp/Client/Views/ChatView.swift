import SwiftUI
import BearsClient

@MainActor
struct ChatView: View {
    @ObservedObject var model: NativeClientModel
    @State private var confirmDeliveryResolution = false

    var body: some View {
        if let selected = model.selectedConversation {
            VStack(spacing: 0) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(selected.title).font(.title2.bold()).textSelection(.enabled)
                        HStack(spacing: 10) {
                            Label(model.connection.label, systemImage: model.connection == .ready ? "circle.fill" : "circle")
                                .foregroundStyle(model.connection == .ready ? Color.green : Color.secondary)
                            Text(model.hatName(selected.hatID))
                            if !selected.canSend { Label("Read-only", systemImage: "lock") }
                        }.font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if model.connection == .connecting { ProgressView().controlSize(.small) }
                    Menu {
                        Button("Refresh canonical history") { model.refreshHistory() }.disabled(!canChangeConnection)
                        Button("Reconnect live chat") { model.reconnect() }
                            .disabled(!canChangeConnection || model.connection == .connecting)
                        Button("Disconnect") { model.disconnectChat() }
                            .disabled(model.connection == .disconnected)
                    } label: { Label("Connection", systemImage: "arrow.triangle.2.circlepath") }
                    .menuStyle(.borderlessButton).fixedSize()
                }.padding(16)
                Divider()
                TranscriptView(model: model)
                Divider()
                VStack(alignment: .leading, spacing: 8) {
                    if !selected.canSend {
                        Text("This conversation is read-only. Start a new conversation with an available hat to chat.")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                    if model.connection == .disconnected {
                        HStack {
                            Text("Reconnect explicitly to continue. No message will be resent.").font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Button("Reconnect") { model.reconnect() }
                                .disabled(model.authentication != .signedIn || !canChangeConnection)
                        }
                    }
                    if let status = model.composer.status {
                        Text(status).font(.caption).foregroundStyle(model.composer.deliveryUncertain ? Color.orange : Color.secondary)
                    }
                    if model.composer.deliveryUncertain {
                        HStack {
                            Button("Refresh history") { model.refreshHistory() }.disabled(model.isLoadingHistory)
                            Button("Resolve pending send…") { confirmDeliveryResolution = true }
                        }.controlSize(.small)
                    }
                    TextEditor(text: $model.composer.draft)
                        .font(.body).frame(minHeight: 72, maxHeight: 140)
                        .padding(6).background(Color(nsColor: .textBackgroundColor))
                        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.secondary.opacity(0.25)))
                        .accessibilityLabel("Message draft")
                        .disabled(!selected.canSend || model.user == nil || model.composer.hasPendingSend)
                    HStack {
                        Text(model.composer.draft.utf8.count > 32 * 1024 ? "Message exceeds the 32 KiB text limit." : "⌘ Return to send · Draft retained until Den accepts")
                            .font(.caption2).foregroundStyle(.secondary)
                        Spacer()
                        Button(model.isStopping ? "Stopping…" : "Stop") { model.stopReply() }
                            .disabled(!model.canStop)
                        Button("Send") { model.send() }.buttonStyle(.borderedProminent)
                            .keyboardShortcut(.return, modifiers: .command).disabled(!model.canSend)
                    }
                    CapabilityNotice()
                }.padding(14)
            }
            .confirmationDialog("Review history before sending again", isPresented: $confirmDeliveryResolution) {
                Button("Keep draft and allow a new send", role: .destructive) { model.composer.resolveUncertainDelivery() }
            } message: {
                Text("The pending message may already exist on Den. Review canonical history first. Sending it again could create a duplicate. This action only unlocks the draft; it sends nothing.")
            }
        } else {
            VStack(spacing: 16) {
                Image(systemName: "bubble.left.and.bubble.right").font(.system(size: 40)).foregroundStyle(.secondary)
                Text("Choose a conversation").font(.title2)
                Text("Continue an existing chat, or choose a hat to start a new one.").foregroundStyle(.secondary)
                CapabilityNotice().frame(maxWidth: 360)
            }.padding(32).frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
    private var canChangeConnection: Bool {
        !model.isLoadingHistory && model.composer.activeTurnID == nil &&
        (!model.composer.hasPendingSend || model.composer.deliveryUncertain)
    }
}
