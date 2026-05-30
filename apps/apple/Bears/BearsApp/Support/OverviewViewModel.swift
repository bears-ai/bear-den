import Foundation
#if os(macOS)
import AppKit
#endif

@MainActor
final class OverviewViewModel: ObservableObject {
    private var hasRefreshedOnce = false

    @Published private(set) var installState: InstallState?
    @Published private(set) var managedAdapterPath: String
    @Published private(set) var latestVersion: String = "Unavailable"
    @Published private(set) var installedVersion: String = "Unavailable"
    @Published private(set) var latestVersionDetails: String = "Unavailable"
    @Published private(set) var installedVersionDetails: String = "Unavailable"
    @Published private(set) var statusText: String = "Not checked"
    @Published private(set) var canUpdate = false
    @Published private(set) var lastError: String?
    @Published private(set) var statusCopied = false
    @Published private(set) var latestVersionCopied = false
    @Published private(set) var installedVersionCopied = false

    private let installManager: AdapterInstallManager
    private let pathProvider: BearsPathResolver

    init(
        installManager: AdapterInstallManager = AdapterInstallManager(),
        pathProvider: BearsPathResolver = BearsPathResolver()
    ) {
        self.installManager = installManager
        self.pathProvider = pathProvider
        self.managedAdapterPath = pathProvider.managedAdapterPath.path
    }

    func refresh() {
        guard !hasRefreshedOnce else {
            refreshManifestAndState()
            return
        }

        hasRefreshedOnce = true
        refreshManifestAndState()
    }

    func refreshManifestAndState() {
        do {
            let manifestVersionResult = installManager.currentManifestVersion()
            let installedInfoResult = Result { try installManager.installedAdapterVersion() }
            let installedInfo = try? installedInfoResult.get()
            let state = try installManager.inspectInstallState()
            installState = state
            latestVersion = Self.manifestVersionDisplay(from: manifestVersionResult)
            installedVersion = state.installedVersion ?? installedInfo?.version ?? "Unavailable"
            latestVersionDetails = Self.manifestVersionDetails(from: manifestVersionResult)
            installedVersionDetails = Self.versionDetails(from: installedInfo)
            statusText = Self.statusText(for: state.lastInstallStatus)
            canUpdate = state.lastInstallStatus == .repairNeeded
            let combinedError = Self.combinedError(
                primary: state.lastError,
                referenceVersionError: Self.errorDescription(from: manifestVersionResult, prefix: "Latest version read failed"),
                installedVersionError: Self.errorDescription(from: installedInfoResult, prefix: "Installed version read failed")
            )
            lastError = installedInfo != nil ? nil : Self.shortVisibleError(from: combinedError)
            if let combinedError, lastError != nil {
                fputs("[Bears][OverviewViewModel][refresh][visibleError] \(combinedError)\n", stderr)
            }
        } catch {
            statusText = "Error"
            canUpdate = false
            lastError = error.localizedDescription
            latestVersion = "Unavailable"
            latestVersionDetails = "Unavailable"
            installedVersionDetails = "Unavailable"
            fputs("[Bears][refresh] \(error.localizedDescription)\n", stderr)
        }
    }

    func updateInstall() {
        do {
            let state = try installManager.updateInstall()
            let manifestVersionResult = installManager.currentManifestVersion()
            let installedInfoResult = Result { try installManager.installedAdapterVersion() }
            let installedInfo = try? installedInfoResult.get()
            installState = state
            latestVersion = Self.manifestVersionDisplay(from: manifestVersionResult)
            installedVersion = state.installedVersion ?? installedInfo?.version ?? "Unavailable"
            latestVersionDetails = Self.manifestVersionDetails(from: manifestVersionResult)
            installedVersionDetails = Self.versionDetails(from: installedInfo)
            statusText = Self.statusText(for: state.lastInstallStatus)
            canUpdate = state.lastInstallStatus == .repairNeeded
            let combinedError = Self.combinedError(
                primary: state.lastError,
                referenceVersionError: Self.errorDescription(from: manifestVersionResult, prefix: "Latest version read failed"),
                installedVersionError: Self.errorDescription(from: installedInfoResult, prefix: "Installed version read failed")
            )
            lastError = installedInfo != nil ? nil : Self.shortVisibleError(from: combinedError)
            if let combinedError, lastError != nil {
                fputs("[Bears][OverviewViewModel][updateInstall][visibleError] \(combinedError)\n", stderr)
            }
        } catch {
            statusText = "Error"
            canUpdate = false
            lastError = error.localizedDescription
            latestVersion = "Unavailable"
            latestVersionDetails = "Unavailable"
            installedVersionDetails = "Unavailable"
            fputs("[Bears][repairInstall] \(error.localizedDescription)\n", stderr)
        }
    }

    private static func versionDetails(from info: AdapterVersionInfo?) -> String {
        guard let info else {
            return "Unavailable"
        }

        return [
            "version=\(info.version)",
            "buildGitSha=\(info.buildGitSha)",
            "localHeadSha=\(info.localHeadSha)",
            "builtAtUTC=\(info.builtAtUtc)",
            "chromeTools=\(info.chromeTools)",
            "directTools=\(info.directTools?.count ?? 0) entries"
        ].joined(separator: "\n")
    }

    private static func manifestVersionDisplay(from result: Result<String?, Error>) -> String {
        switch result {
        case .success(let version):
            return version ?? "Unavailable"
        case .failure(let error as GitHubReleaseAdapterSourceError):
            switch error {
            case .manifestNotFound:
                return "Not Found"
            case .manifestUnavailable, .invalidManifestJSON:
                return "Error"
            default:
                return "Unavailable"
            }
        case .failure:
            return "Error"
        }
    }

    private static func manifestVersionDetails(from result: Result<String?, Error>) -> String {
        switch result {
        case .success(let version):
            return version.map { "version=\($0)" } ?? "Latest version unavailable from manifest"
        case .failure(let error):
            return "Latest version unavailable: \(error.localizedDescription)"
        }
    }

    private static func errorDescription<T>(from result: Result<T, Error>, prefix: String) -> String? {
        switch result {
        case .success:
            return nil
        case .failure(let error):
            return "\(prefix): \(error.localizedDescription)"
        }
    }

    func versionDetails(forInstalledVersion: Bool) -> String {
        forInstalledVersion ? installedVersionDetails : latestVersionDetails
    }

    func copyManagedAdapterPath() {
        #if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(managedAdapterPath, forType: .string)
        #endif

        statusCopied = true
        Task {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            statusCopied = false
        }
    }

    func copyVersionDetails(forInstalledVersion: Bool) {
        #if os(macOS)
        let details = versionDetails(forInstalledVersion: forInstalledVersion)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(details, forType: .string)
        #endif

        if forInstalledVersion {
            installedVersionCopied = true
        } else {
            latestVersionCopied = true
        }

        Task {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            if forInstalledVersion {
                installedVersionCopied = false
            } else {
                latestVersionCopied = false
            }
        }
    }

    private static func combinedError(primary: String?, referenceVersionError: String?, installedVersionError: String?) -> String? {
        let parts = [primary, referenceVersionError, installedVersionError].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: "\n")
    }

    private static func shortVisibleError(from error: String?) -> String? {
        guard let error, !error.isEmpty else {
            return nil
        }

        let firstLine = error
            .split(separator: "\n", omittingEmptySubsequences: false)
            .first
            .map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines)

        guard let firstLine, !firstLine.isEmpty else {
            return "Error details available"
        }

        return firstLine.count > 160 ? String(firstLine.prefix(157)) + "..." : firstLine
    }

    private static func statusText(for status: InstallStatus) -> String {
        switch status {
        case .ok:
            return "Up to Date"
        case .missing:
            return "Not Installed"
        case .repairNeeded:
            return "Needs Update"
        case .error:
            return "Needs Update"
        }
    }
}
