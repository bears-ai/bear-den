import Foundation

protocol AdapterArtifactDownloading {
    func downloadArtifact(from source: AdapterArtifactSource) throws -> URL
}

struct DownloadedAdapterArtifact {
    let localURL: URL
    let source: AdapterArtifactSource
}

enum AdapterArtifactDownloadError: LocalizedError {
    case missingDownloadedFile(URL)
    case invalidExecutableFormat(String)

    var errorDescription: String? {
        switch self {
        case .missingDownloadedFile(let url):
            return "Downloaded adapter artifact could not be found at \(url.path)."
        case .invalidExecutableFormat(let details):
            return details
        }
    }
}

struct URLSessionAdapterArtifactDownloader: AdapterArtifactDownloading {
    private let session: URLSession
    private let fileManager: FileManager

    init(session: URLSession = .shared, fileManager: FileManager = .default) {
        self.session = session
        self.fileManager = fileManager
    }

    func downloadArtifact(from source: AdapterArtifactSource) throws -> URL {
        let temporaryDirectory = fileManager.temporaryDirectory
            .appendingPathComponent("BearsAdapterDownload", isDirectory: true)
        try ensureCleanDirectory(at: temporaryDirectory)

        let downloadedURL = try downloadSynchronously(from: source.downloadURL, into: temporaryDirectory)
        let resolvedURL = try resolveDownloadedBinary(from: downloadedURL, source: source, in: temporaryDirectory)
        if !source.isInstallerPackage {
            try validateDownloadedBinary(at: resolvedURL)
        }
        return resolvedURL
    }

    private func downloadSynchronously(from remoteURL: URL, into directory: URL) throws -> URL {
        let semaphore = DispatchSemaphore(value: 0)
        var result: Result<URL, Error>!

        let task = session.downloadTask(with: remoteURL) { temporaryURL, _, error in
            defer { semaphore.signal() }

            if let error {
                result = .failure(error)
                return
            }

            guard let temporaryURL else {
                result = .failure(AdapterArtifactDownloadError.missingDownloadedFile(remoteURL))
                return
            }

            let destinationURL = directory.appendingPathComponent(remoteURL.lastPathComponent, isDirectory: false)
            do {
                if self.fileManager.fileExists(atPath: destinationURL.path) {
                    try self.fileManager.removeItem(at: destinationURL)
                }
                try self.fileManager.moveItem(at: temporaryURL, to: destinationURL)
                result = .success(destinationURL)
            } catch {
                result = .failure(error)
            }
        }

        task.resume()
        semaphore.wait()
        return try result.get()
    }

    private func resolveDownloadedBinary(from downloadedURL: URL, source: AdapterArtifactSource, in directory: URL) throws -> URL {
        if source.isInstallerPackage || downloadedURL.pathExtension == "pkg" {
            guard fileManager.fileExists(atPath: downloadedURL.path) else {
                throw AdapterArtifactDownloadError.missingDownloadedFile(downloadedURL)
            }
            return downloadedURL
        }

        if fileManager.fileExists(atPath: downloadedURL.path) {
            return downloadedURL
        }

        let fallbackURL = directory.appendingPathComponent(source.assetName, isDirectory: false)
        guard fileManager.fileExists(atPath: fallbackURL.path) else {
            throw AdapterArtifactDownloadError.missingDownloadedFile(downloadedURL)
        }
        return fallbackURL
    }

    private func validateDownloadedBinary(at url: URL) throws {
        let data = try Data(contentsOf: url, options: [.mappedIfSafe])
        guard data.count >= 4 else {
            throw AdapterArtifactDownloadError.invalidExecutableFormat("Downloaded adapter artifact is too small to be a valid executable.")
        }

        let bytes = [UInt8](data.prefix(4))
        let isMachO =
            bytes == [0xCF, 0xFA, 0xED, 0xFE] ||
            bytes == [0xFE, 0xED, 0xFA, 0xCF] ||
            bytes == [0xCA, 0xFE, 0xBA, 0xBE] ||
            bytes == [0xBE, 0xBA, 0xFE, 0xCA]

        guard isMachO else {
            throw AdapterArtifactDownloadError.invalidExecutableFormat(
                "Downloaded adapter artifact is not a macOS Mach-O executable. Build and publish a macOS adapter binary, or set BEARS_ADAPTER_DOWNLOAD_URL to a valid macOS artifact."
            )
        }
    }

    private func ensureCleanDirectory(at directory: URL) throws {
        if fileManager.fileExists(atPath: directory.path) {
            try fileManager.removeItem(at: directory)
        }
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
    }
}
