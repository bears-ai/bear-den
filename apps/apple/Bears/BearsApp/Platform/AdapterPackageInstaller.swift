import Foundation
#if os(macOS)
import AppKit
#endif

protocol AdapterPackageInstalling {
    func installPackage(at packageURL: URL) throws -> String
}

enum AdapterPackageInstallerError: LocalizedError {
    case installerFailed(String)

    var errorDescription: String? {
        switch self {
        case .installerFailed(let message):
            return message
        }
    }
}

struct InstallerAppAdapterPackageInstaller: AdapterPackageInstalling {
    func installPackage(at packageURL: URL) throws -> String {
        #if os(macOS)
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true

        var openError: Error?
        let semaphore = DispatchSemaphore(value: 0)

        NSWorkspace.shared.open([packageURL], withApplicationAt: URL(fileURLWithPath: "/System/Library/CoreServices/Installer.app"), configuration: configuration) { _, error in
            openError = error
            semaphore.signal()
        }

        semaphore.wait()

        if let openError {
            throw AdapterPackageInstallerError.installerFailed("Failed to open the adapter package in Installer.app: \(openError.localizedDescription)")
        }

        return "Opened adapter package in Installer.app."
        #else
        throw AdapterPackageInstallerError.installerFailed("Opening Installer.app is only supported on macOS.")
        #endif
    }
}
