import SwiftUI

@main
@MainActor
struct BearsApp: App {
    @StateObject private var client = NativeClientModel.live()

    var body: some Scene {
        Window("Bears", id: "bears-client") {
            if Self.isSupportedArchitecture {
                NativeClientView(model: client)
            } else {
                UnsupportedArchitectureView()
            }
        }
        .commands { ClientCommands(model: client) }

        Settings {
            if Self.isSupportedArchitecture {
                TabView {
                    ConnectionSettingsView(model: client)
                        .tabItem { Label("Connection", systemImage: "network") }
                    OverviewView()
                        .tabItem { Label("Armature", systemImage: "wrench.and.screwdriver") }
                }
                .frame(width: 720, height: 650)
            } else {
                UnsupportedArchitectureView()
            }
        }
    }

    private static var isSupportedArchitecture: Bool {
        #if arch(arm64)
        true
        #else
        false
        #endif
    }
}

private struct UnsupportedArchitectureView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Bears")
                .font(.largeTitle)
                .bold()

            Text("This preview app currently supports only Apple Silicon Macs.")
                .font(.headline)

            Text("Please run Bears on an arm64 Mac. Intel macOS support has not been implemented yet.")
                .textSelection(.enabled)
        }
        .padding(20)
        .frame(minWidth: 520, minHeight: 220)
    }
}
