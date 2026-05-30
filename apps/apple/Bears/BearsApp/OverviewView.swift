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
                    keyValueRow("Installed Version", value: viewModel.installedVersion)

                    if let error = viewModel.lastError, !error.isEmpty {
                        Text(error)
                            .font(.caption)
                            .foregroundStyle(.red)
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
}
