import SoundroomCore
import SwiftUI

enum EffectSymbol {
    static func of(_ rawValue: String) -> String {
        switch rawValue {
        case "normalize": return "waveform.path.badge.plus"
        case "gain": return "plusminus"
        case "fadeIn": return "chart.line.uptrend.xyaxis"
        case "fadeOut": return "chart.line.downtrend.xyaxis"
        case "reverse": return "backward.end"
        case "gate": return "door.left.hand.closed"
        case "guitarRig": return "guitars"
        case "utility": return "arrow.left.arrow.right"
        case "compressor": return "arrow.down.right.and.arrow.up.left"
        case "lowPass": return "line.3.horizontal.decrease"
        case "highPass": return "line.3.horizontal.decrease.circle"
        case "equalizer": return "slider.vertical.3"
        case "limiter": return "waveform.badge.exclamationmark"
        case "resonance": return "waveform.path.ecg"
        case "vocalEnhancer": return "mic.badge.plus"
        case "soundPutty": return "waveform.path"
        case "sourceIsolator": return "line.3.horizontal.decrease.circle.fill"
        case "warmth": return "flame"
        case "echo": return "repeat"
        case "room": return "cube.transparent"
        case "chorus": return "waveform.path"
        case "pitchCorrection": return "music.note"
        case "dither": return "square.grid.3x3"
        default: return "waveform"
        }
    }
}

enum EffectFamily: String, CaseIterable, Identifiable {
    case level = "Level"
    case tone = "Tone"
    case space = "Space"
    case pitch = "Pitch"
    case utility = "Utility"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .level: return "waveform.path"
        case .tone: return "line.3.horizontal.decrease"
        case .space: return "waveform.path.ecg"
        case .pitch: return "music.note"
        case .utility: return "wand.and.stars"
        }
    }
    var color: Color {
        switch self {
        case .level: return Color(red: 0.65, green: 0.74, blue: 0.78)
        case .tone: return Color(red: 0.71, green: 0.68, blue: 0.83)
        case .space: return Color(red: 0.57, green: 0.72, blue: 0.82)
        case .pitch: return Color(red: 0.78, green: 0.73, blue: 0.61)
        case .utility: return Theme.secondary
        }
    }
    static func of(_ rawValue: String) -> Self {
        switch rawValue {
        case "normalize", "gain", "fadeIn", "fadeOut", "compressor", "gate", "limiter": return .level
        case "lowPass", "highPass", "equalizer", "warmth", "resonance", "vocalEnhancer", "sourceIsolator", "soundPutty",
            "guitarRig":
            return .tone
        case "echo", "room", "chorus": return .space
        case "pitchCorrection": return .pitch
        default: return .utility
        }
    }
}

/// Shared browser for rendered selection effects and live track inserts.
struct EffectBrowser: View {
    struct Entry: Identifiable {
        let id: String
        let name: String
        var family: EffectFamily { .of(id) }
    }
    let entries: [Entry]
    var selected: String? = nil
    let onSelect: (String) -> Void
    @State private var search = ""
    @State private var family: EffectFamily?
    @FocusState private var searchFocused: Bool
    @Environment(\.dismiss) private var dismiss
    private var matches: [Entry] {
        entries.filter { entry in
            (family == nil || entry.family == family)
                && (search.isEmpty || entry.name.localizedCaseInsensitiveContains(search)
                    || entry.family.rawValue.localizedCaseInsensitiveContains(search))
        }
    }
    var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 7) {
                Image(systemName: "magnifyingglass").foregroundStyle(Theme.secondary)
                TextField("Search effects", text: $search).textFieldStyle(.plain).focused($searchFocused)
                    .onSubmit { if let entry = matches.first { choose(entry) } }
                if !search.isEmpty {
                    Button {
                        search = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                    }.buttonStyle(.plain)
                        .accessibilityLabel("Clear effect search")
                }
            }.padding(9).background(Theme.background, in: RoundedRectangle(cornerRadius: 6))
            Picker("Category", selection: $family) {
                Text("All effects").tag(Optional<EffectFamily>.none)
                ForEach(EffectFamily.allCases) { category in
                    Label(category.rawValue, systemImage: category.icon).tag(Optional(category))
                }
            }.labelsHidden().accessibilityLabel("Effect category")
            ScrollView {
                LazyVStack(spacing: 3) {
                    ForEach(matches) { entry in
                        Button {
                            choose(entry)
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: EffectSymbol.of(entry.id)).foregroundStyle(entry.family.color).frame(
                                    width: 22, height: 26)
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(entry.name).fontWeight(.medium)
                                    if let effect = BuiltInEffect(rawValue: entry.id) {
                                        Text(effect.detail).font(.system(size: 10)).foregroundStyle(Theme.secondary)
                                            .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                                    }
                                }.frame(maxWidth: .infinity, alignment: .leading)
                                if entry.id == selected { Image(systemName: "checkmark").foregroundStyle(Theme.accent) }
                            }.padding(.horizontal, 9).padding(.vertical, 9).contentShape(Rectangle())
                                .background(
                                    entry.id == selected ? Theme.accent.opacity(0.09) : Color.clear,
                                    in: RoundedRectangle(cornerRadius: 5))
                        }.buttonStyle(.plain).accessibilityAddTraits(entry.id == selected ? .isSelected : [])
                    }
                    if matches.isEmpty { Text("No effects found").foregroundStyle(Theme.secondary).padding(20) }
                }
            }
        }.font(.system(size: 12)).padding(12).frame(width: 290, height: 370)
            .foregroundStyle(Theme.ink).background(Theme.paper).preferredColorScheme(.dark)
            .onAppear { searchFocused = true }.onExitCommand { dismiss() }
    }
    private func choose(_ entry: Entry) {
        onSelect(entry.id)
        dismiss()
    }
}

struct EffectIdentity: View {
    let name: String
    let rawValue: String
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: EffectSymbol.of(rawValue)).foregroundStyle(EffectFamily.of(rawValue).color).frame(
                width: 18)
            Text(name).fontWeight(.semibold)
        }.font(.system(size: 12))
    }
}

enum EffectDrawerLayout {
    static let minimumTimeline: CGFloat = 620
    static let minimumWidth: CGFloat = 280
    static let maximumWidth: CGFloat = 460
    static let dividerWidth: CGFloat = 6
    static func width(preferred: Double, available: CGFloat) -> CGFloat {
        let maximum = min(maximumWidth, max(minimumWidth, available - minimumTimeline - dividerWidth))
        return min(maximum, max(minimumWidth, preferred.isFinite ? CGFloat(preferred) : 320))
    }
}

/// The popover's native window can still be closing when SwiftUI disappears it.
@MainActor func restoreEditorAfterEffectBrowser(_ store: SessionStore) {
    DispatchQueue.main.async { store.focusEditor() }
}
