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
                    keyValueRow("Status", value: viewModel.statusText)
                    keyValueRow("Managed Path", value: viewModel.managedAdapterPath)
                    versionRow(
                        "Bundled Version",
                        value: viewModel.bundledVersion,
                        details: viewModel.versionDetails(forInstalledVersion: false),
                        copied: viewModel.bundledVersionCopied,
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
                    viewModel.refresh()
                }

                Button("Repair Installation") {
                    viewModel.repairInstall()
                }

                Button("Copy Path") {
                    #if os(macOS)
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(viewModel.managedAdapterPath, forType: .string)
                    #endif
                }
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
    private func keyValueRow(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.headline)
            Text(value)
                .font(.body.monospaced())
                .textSelection(.enabled)
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
