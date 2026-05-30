import SwiftUI
#if os(macOS)
import AppKit
#endif

struct OverviewView: View {
    @StateObject private var viewModel = OverviewViewModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Bears")
                .font(.largeTitle)
                .bold()

            GroupBox("Adapter Status") {
                VStack(alignment: .leading, spacing: 10) {
                    statusRow(
                        value: viewModel.statusText,
                        path: viewModel.managedAdapterPath,
                        copied: viewModel.statusCopied,
                        action: { viewModel.copyManagedAdapterPath() }
                    )
                    versionRow(
                        "Latest Version",
                        value: viewModel.latestVersion,
                        details: viewModel.versionDetails(forInstalledVersion: false),
                        copied: viewModel.latestVersionCopied,
                        action: { viewModel.copyVersionDetails(forInstalledVersion: false) }
                    )
                    versionRow(
                        "Installed Version",
                        value: viewModel.installedVersion,
                        details: viewModel.versionDetails(forInstalledVersion: true),
                        copied: viewModel.installedVersionCopied,
                        action: { viewModel.copyVersionDetails(forInstalledVersion: true) }
                    )

                    if let error = viewModel.lastError, !error.isEmpty {
                        Button {
                            #if os(macOS)
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(error, forType: .string)
                            #endif
                        } label: {
                            Text(error)
                                .font(.caption.monospaced())
                                .foregroundStyle(.red)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(.plain)
                        .help(error + "\n\nClick to copy full error to clipboard.")
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            HStack {
                Button("Refresh") {
                    viewModel.refreshManifestAndState()
                }

                Button("Update") {
                    viewModel.updateInstall()
                }
                .disabled(!viewModel.canUpdate)

            }

            Spacer()
        }
        .padding(20)
        .frame(minWidth: 640, minHeight: 360)
        .onAppear {
            viewModel.refresh()
        }
    }

    @ViewBuilder
    private func statusRow(
        value: String,
        path: String,
        copied: Bool,
        action: @escaping () -> Void
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Status")
                .font(.headline)
            Button(action: action) {
                Text(copied ? "path copied" : value)
                    .font(.body.monospaced())
                    .foregroundStyle(copied ? .secondary : .primary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .help(path + "\n\nClick to copy managed adapter path to clipboard.")
        }
    }

    @ViewBuilder
    private func versionRow(
        _ label: String,
        value: String,
        details: String,
        copied: Bool,
        action: @escaping () -> Void
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.headline)
            Button(action: action) {
                Text(copied ? "details copied" : value)
                    .font(.body.monospaced())
                    .foregroundStyle(copied ? .secondary : .primary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .help(details + "\n\nClick to copy full details to clipboard.")
        }
    }
}
