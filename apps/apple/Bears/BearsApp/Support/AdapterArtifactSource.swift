import Foundation

struct AdapterArtifactSource: Equatable {
    let downloadURL: URL
    let versionHint: String?
    let assetName: String
    let isInstallerPackage: Bool
}

protocol AdapterArtifactSourceProviding {
    func latestMacOSArtifactSource() throws -> AdapterArtifactSource
}
