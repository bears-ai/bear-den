import SwiftUI
import BearsClient

@MainActor
struct TranscriptView: View {
    @ObservedObject var model: NativeClientModel
    @State private var followReplies = true

    var body: some View {
        ScrollViewReader { reader in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 20) {
                    if !model.hasSnapshot {
                        HStack {
                            Text("Canonical saved history").font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            if model.historyHasMore {
                                Button("Load older messages") { model.loadOlderHistory() }.disabled(model.isLoadingHistory)
                            }
                        }
                        ForEach(model.history) { message in
                            messageRow(author: author(message.role), text: message.text, isHuman: message.role == .user)
                        }
                    } else if let transcript = model.transcript {
                        ForEach(transcript.turns) { turn in
                            VStack(alignment: .leading, spacing: 16) {
                                messageRow(author: "Human", text: turn.message.text, isHuman: true)
                                ForEach(Array(turn.responseParts.enumerated()), id: \.offset) { _, part in
                                    switch part {
                                    case .markdown(let markdown):
                                        if !markdown.content.isEmpty { messageRow(author: model.selectedBear?.name ?? "Bear", text: markdown.content, isHuman: false) }
                                    case .error:
                                        Label("The reply failed. Refresh saved history before sending again.", systemImage: "exclamationmark.circle")
                                            .font(.caption).foregroundStyle(.secondary)
                                    case .unsupported:
                                        Text("Unsupported response content omitted.").font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                                if turn.state == .cancelled {
                                    Text("Reply stopped. Partial text may have been saved.").font(.caption).foregroundStyle(.secondary)
                                } else if turn.state == .streaming && model.connection == .disconnected {
                                    Text("Connection ended. Refresh saved history to reconcile this reply.").font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                        if transcript.unsupportedEventCount > 0 {
                            Text("\(transcript.unsupportedEventCount) unsupported server event(s) ignored. This client exposes text chat only.")
                                .font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    if model.isLoadingHistory { ProgressView().controlSize(.small).frame(maxWidth: .infinity) }
                    if !model.isLoadingHistory && isEmpty {
                        Text("No messages yet.").foregroundStyle(.secondary).frame(maxWidth: .infinity).padding(.vertical, 40)
                    }
                    Color.clear.frame(height: 1).id("transcript-bottom")
                }.padding(20).frame(maxWidth: 860, alignment: .leading).frame(maxWidth: .infinity)
            }
            .safeAreaInset(edge: .top, alignment: .trailing, spacing: 0) {
                Toggle("Follow replies", isOn: $followReplies).toggleStyle(.checkbox)
                    .font(.caption).padding(.horizontal, 16).padding(.vertical, 6)
                    .background(.regularMaterial)
            }
            .onChange(of: model.transcript?.lastServerSeq) { _ in
                if followReplies && model.hasSnapshot { reader.scrollTo("transcript-bottom", anchor: .bottom) }
            }
            .onChange(of: model.hasSnapshot) { _ in reader.scrollTo("transcript-bottom", anchor: .bottom) }
            .onChange(of: model.composer.hasPendingSend) { pending in
                if !pending { reader.scrollTo("transcript-bottom", anchor: .bottom) }
            }
        }
    }
    private var isEmpty: Bool { model.hasSnapshot ? model.transcript?.turns.isEmpty ?? true : model.history.isEmpty }
    private func author(_ role: HistoryRole) -> String {
        switch role {
        case .user: return "Human"
        case .assistant: return model.selectedBear?.name ?? "Bear"
        case .system: return "System"
        case .tool: return "Tool result"
        case .unsupported: return "Other message"
        }
    }
    private func messageRow(author: String, text: String, isHuman: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(author).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            MarkdownMessage(text: text)
        }
        .padding(14).frame(maxWidth: .infinity, alignment: .leading)
        .background(isHuman ? Color.accentColor.opacity(0.07) : Color.secondary.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))
    }
}

private struct MarkdownMessage: View {
    let text: String
    var body: some View {
        Text((try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(text))
            .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
    }
}
