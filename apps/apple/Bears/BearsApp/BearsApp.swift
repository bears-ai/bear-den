import SwiftUI

@main
struct BearsApp: App {
    var body: some Scene {
        WindowGroup {
            if Self.isSupportedArchitecture {
                OverviewView()
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
