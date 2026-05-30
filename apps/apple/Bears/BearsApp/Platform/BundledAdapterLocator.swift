import Foundation

protocol BundledAdapterLocating {
    func bundledAdapterExecutableURL() throws -> URL
}

enum BundledAdapterLocatorError: LocalizedError {
    case missingResource(checkedPaths: [String])

    var errorDescription: String? {
        switch self {
        case .missingResource(let checkedPaths):
            let details = checkedPaths.isEmpty ? "No candidate paths were checked." : checkedPaths.joined(separator: "\n")
            return "The Bears app bundle does not contain a bundled bears-acp-adapter executable yet. Checked:\n\(details)"
        }
    }
}

struct BundledAdapterLocator: BundledAdapterLocating {
    func bundledAdapterExecutableURL() throws -> URL {
        let candidates = [
            Bundle.module.url(forResource: "bears-acp-adapter", withExtension: nil),
            Bundle.module.url(forResource: "bears-acp-adapter", withExtension: nil, subdirectory: "Adapter"),
            Bundle.module.url(forResource: "bears-acp-adapter", withExtension: nil, subdirectory: "Resources/Adapter"),
            Bundle.module.resourceURL?
                .appendingPathComponent("Adapter", isDirectory: true)
                .appendingPathComponent("bears-acp-adapter", isDirectory: false),
            Bundle.module.resourceURL?
                .appendingPathComponent("Resources", isDirectory: true)
                .appendingPathComponent("Adapter", isDirectory: true)
                .appendingPathComponent("bears-acp-adapter", isDirectory: false)
        ]

        for case let url? in candidates where FileManager.default.fileExists(atPath: url.path) {
            return url
        }

        let checkedPaths = candidates.map { $0?.path ?? "<nil>" }
        throw BundledAdapterLocatorError.missingResource(checkedPaths: checkedPaths)
    }
}
