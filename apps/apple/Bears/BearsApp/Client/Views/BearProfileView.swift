import SwiftUI
import BearsClient

@MainActor
struct BearProfileView: View {
    @ObservedObject var model: NativeClientModel
    @State private var confirmAbandon = false

    var body: some View {
        if let bear = model.selectedBear {
            VStack(alignment: .leading, spacing: 0) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(bear.name).font(.title.bold()).textSelection(.enabled)
                        Label(bear.isBearAdmin ? "Bear administrator" : "Member", systemImage: bear.isBearAdmin ? "checkmark.shield" : "person")
                            .font(.caption).foregroundStyle(.secondary)
                        if !bear.description.isEmpty { Text(bear.description).textSelection(.enabled) }
                        DisclosureGroup("Shared hats (\(model.hats.count))") {
                            VStack(alignment: .leading, spacing: 12) {
                                ForEach(model.hats) { hat in
                                    VStack(alignment: .leading, spacing: 3) {
                                        Text(hat.name).font(.subheadline.bold())
                                        if let summary = hat.shortSummary, !summary.isEmpty {
                                            Text(summary).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                                        }
                                    }
                                }
                                if model.hats.isEmpty { Text("No hats are available.").font(.caption).foregroundStyle(.secondary) }
                            }.padding(.top, 8).frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
                }.frame(maxHeight: 285)
                Divider()
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("Conversations").font(.headline)
                        Spacer()
                        Button { model.refreshInventory() } label: { Image(systemName: "arrow.clockwise") }
                            .help("Refresh conversations and hats").disabled(model.isLoadingInventory || model.isCreating)
                    }
                    Picker("New chat hat", selection: $model.newHatID) {
                        Text("Choose a hat…").tag(nil as HatID?)
                        ForEach(model.hats) { hat in Text(hat.name).tag(Optional(hat.id)) }
                    }
                    .disabled(model.pendingCreation != nil || model.isCreating)
                    HStack {
                        Button(model.pendingCreation == nil ? "New conversation" : "Retry same creation") { model.createConversation() }
                            .disabled(model.isCreating || (model.pendingCreation == nil && model.newHatID == nil))
                        if model.isCreating { ProgressView().controlSize(.small) }
                        if model.pendingCreation != nil && !model.isCreating {
                            Button("Dismiss request") { confirmAbandon = true }.controlSize(.small)
                        }
                    }
                    if model.pendingCreation != nil {
                        Text("A retry uses the same request UUID and hat; it does not create a new command.")
                            .font(.caption2).foregroundStyle(.secondary)
                    }
                }.padding(12)
                if model.isLoadingInventory { ProgressView().controlSize(.small).padding(8) }
                List(selection: Binding(get: { model.selectedConversation?.id }, set: selectConversation)) {
                    ForEach(model.conversations) { conversation in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(conversation.title).lineLimit(2)
                                Spacer()
                                if !conversation.canSend { Image(systemName: "lock").help("Read-only") }
                            }
                            Text(model.hatName(conversation.hatID)).font(.caption).foregroundStyle(.secondary)
                            if let date = ClientDate.parse(conversation.lastMessageAt) {
                                Text(date, style: .date).font(.caption2).foregroundStyle(.secondary)
                            }
                        }
                        .padding(.vertical, 4).tag(conversation.id)
                        .accessibilityLabel(conversation.title + (conversation.canSend ? "" : ", read-only"))
                    }
                }
                if model.conversations.isEmpty && !model.isLoadingInventory {
                    Text("Choose a hat to start a conversation.").font(.caption).foregroundStyle(.secondary).padding()
                }
            }
            .confirmationDialog("Dismiss this creation request?", isPresented: $confirmAbandon) {
                Button("Dismiss request", role: .destructive) { model.abandonCreation() }
            } message: {
                Text("The request may already have succeeded. Refresh conversations before starting another request. Nothing will be retried automatically.")
            }
        } else {
            VStack(spacing: 12) {
                Image(systemName: "person.crop.circle").font(.largeTitle)
                Text("Choose a Bear").font(.title2)
                Text("Only Bears you belong to are listed.").foregroundStyle(.secondary)
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
    private func selectConversation(_ id: ConversationID?) {
        guard let id, let conversation = model.conversations.first(where: { $0.id == id }) else { return }
        model.selectConversation(NativeConversation(conversation))
    }
}

enum ClientDate {
    static func parse(_ value: String?) -> Date? {
        guard let value else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: value)
    }
}
