import Foundation

struct SemanticVersion: Equatable {
    let major: Int
    let minor: Int
    let patch: Int

    init?(parsing versionString: String) {
        let core = versionString
            .split(separator: "+", maxSplits: 1, omittingEmptySubsequences: false)
            .first?
            .split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
            .first ?? Substring(versionString)

        let parts = core.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count >= 2 else {
            return nil
        }

        guard
            let major = Int(parts[0]),
            let minor = Int(parts[1])
        else {
            return nil
        }

        let patch = parts.count >= 3 ? Int(parts[2]) ?? 0 : 0

        self.major = major
        self.minor = minor
        self.patch = patch
    }

    func isCompatiblePatchwise(with other: SemanticVersion) -> Bool {
        major == other.major && minor == other.minor
    }
}
