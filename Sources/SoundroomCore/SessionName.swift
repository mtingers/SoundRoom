import Foundation

public enum SessionName {
    public static func validated(_ value: String) throws -> String {
        let name = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw SessionNameError.empty }
        guard name.utf8.count <= 128 else { throw SessionNameError.tooLong }
        guard !name.hasPrefix("."), !name.contains("/"), !name.contains("\\"), !name.contains(":"),
              name.rangeOfCharacter(from: .controlCharacters) == nil else { throw SessionNameError.invalid }
        return name
    }
    public static func projectTitle(_ value: String) throws -> String {
        let name = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return try validated(name.hasSuffix(".soundroom") ? String(name.dropLast(10)) : name)
    }
}
public enum SessionNameError: LocalizedError {
    case empty, tooLong, invalid
    public var errorDescription: String? {
        switch self {
        case .empty: return "Enter a session name."
        case .tooLong: return "The session name is too long. Use a shorter name."
        case .invalid: return "Use a name without /, \\, : or control characters, and without a leading dot."
        }
    }
}
