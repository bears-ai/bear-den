import Foundation

enum GitHubReleaseAdapterSourceError: LocalizedError {
    case invalidURL(String)

    var errorDescription: String? {
        switch self {
        case .invalidURL(let urlString):
            return "Invalid adapter download URL: \(urlString)"
        }
    }
}

struct GitHubReleaseAdapterSource: AdapterArtifactSourceProviding {
    private let environment: ProcessInfo

    init(environment: ProcessInfo = .processInfo) {
        self.environment = environment
    }

    func latestMacOSArtifactSource() throws -> AdapterArtifactSource {
        let configuredURLString = environment.environment["BEARS_ADAPTER_DOWNLOAD_URL"]
            ?? "https://bears-ai.github.io/bear-den/bears-acp-adapter/stable/bears-acp-adapter-aarch64-apple-darwin.pkg"

        guard let downloadURL = URL(string: configuredURLString) else {
            throw GitHubReleaseAdapterSourceError.invalidURL(configuredURLString)
        }

        return AdapterArtifactSource(
            downloadURL: downloadURL,
            versionHint: nil,
            assetName: downloadURL.lastPathComponent.isEmpty ? "bears-acp-adapter-aarch64-apple-darwin.pkg" : downloadURL.lastPathComponent,
            isInstallerPackage: downloadURL.pathExtension == "pkg"
        )
    }
}
