import SwiftUI
import AppKit
import BearsClient

@MainActor
struct NativeClientView: View {
    @ObservedObject var model: NativeClientModel

    var body: some View {
        VStack(spacing: 0) {
            if model.failedSignOut != nil {
                HStack {
                    Label("A saved credential could not be removed from Keychain.", systemImage: "exclamationmark.triangle")
                    Spacer()
                    Button("Retry removal") { model.retryKeychainRemoval() }
                }
                .padding(10).background(Color.orange.opacity(0.12))
            }
            if model.authentication == .signedIn {
                signedInContent
            } else {
                HStack(spacing: 0) {
                    VStack(alignment: .leading, spacing: 20) {
                        Image(systemName: "bubble.left.and.bubble.right").font(.system(size: 44)).foregroundStyle(.tint)
                        Text("Bears").font(.largeTitle.bold())
                        Text("Your Bears, in conversation.").font(.title2)
                        Text("Connect to your Den, choose a Bear and a hat, and continue a conversation.")
                            .foregroundStyle(.secondary)
                        Spacer()
                        CapabilityNotice()
                    }
                    .padding(36).frame(maxWidth: 340, maxHeight: .infinity, alignment: .topLeading)
                    Divider()
                    ConnectionSettingsView(model: model)
                }
            }
        }
        .frame(minWidth: 1050, minHeight: 680)
        .onAppear { model.start() }
        .onDisappear { model.disconnectChat() }
    }
    private var signedInContent: some View {
        VStack(spacing: 0) {
            if let issue = model.issue {
                HStack(alignment: .top) {
                    Image(systemName: "exclamationmark.circle")
                    Text(issue).textSelection(.enabled)
                    Spacer()
                    Button { model.issue = nil } label: { Image(systemName: "xmark") }
                        .buttonStyle(.plain).help("Dismiss message")
                }
                .font(.callout).padding(10).background(Color.orange.opacity(0.1))
            }
            NavigationSplitView {
                VStack(spacing: 0) {
                    List(selection: Binding(get: { model.selectedBearID }, set: { model.selectBear($0) })) {
                        Section("Your Bears") {
                            ForEach(model.bears) { bear in
                                Label(bear.name, systemImage: "person.crop.circle").tag(bear.id)
                            }
                        }
                    }
                    if model.isLoadingMembership { ProgressView().controlSize(.small).padding() }
                    if !model.isLoadingMembership && model.bears.isEmpty {
                        Text("No Bear memberships are available.").font(.caption).foregroundStyle(.secondary).padding()
                    }
                    Divider()
                    VStack(alignment: .leading, spacing: 8) {
                        Label(model.accountName, systemImage: "person.crop.circle.fill").lineLimit(1)
                        HStack {
                            Button("Refresh") { model.refreshMembership() }.disabled(model.isLoadingMembership)
                            Spacer()
                            Button("Sign out") { model.signOut() }
                        }
                        .controlSize(.small)
                    }.padding(12)
                }
                .navigationSplitViewColumnWidth(min: 170, ideal: 210, max: 280)
            } content: {
                BearProfileView(model: model)
                    .navigationSplitViewColumnWidth(min: 270, ideal: 330, max: 440)
            } detail: {
                ChatView(model: model)
            }
            .navigationSplitViewStyle(.balanced)
        }
        .toolbar {
            ToolbarItem {
                Button {
                    NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
                } label: { Label("Settings", systemImage: "gearshape") }
                .help("Connection and armature settings")
            }
        }
    }
}

struct CapabilityNotice: View {
    var body: some View {
        Text("Text chat only. Permissions, subagents, attachments, local tools and a local sandbox are not available in this client.")
            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}
