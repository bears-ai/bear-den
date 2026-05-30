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
                    keyValueRow("Bundled Version", value: viewModel.bundledVersion)
                    keyValueRow("Bundled Version Details", value: viewModel.bundledVersionDetails)
                    keyValueRow("Installed Version", value: viewModel.installedVersion)
                    keyValueRow("Installed Version Details", value: viewModel.installedVersionDetails)

                    if let error = viewModel.lastError, !error.isEmpty {
                        Button {
                            #if os(macOS)
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(error, forType: .string)
                            #endif
                        } label: {
                            Text(shortErrorSummary(error))
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

    private func shortErrorSummary(_ error: String) -> String {
        let firstLine = error
            .split(separator: "\n", omittingEmptySubsequences: false)
            .first
            .map(String.init) ?? error
        let trimmed = firstLine.trimmingCharacters(in: .whitespacesAndNewlines)
        let summary = trimmed.isEmpty ? "Error details available" : trimmed
        return summary.count > 160 ? String(summary.prefix(157)) + "..." : summary
    }
}
