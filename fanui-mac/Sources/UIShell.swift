//! Shared window chrome: tab bar, root layout and the toast stack.

import SwiftUI

// MARK: - window scaffolding

struct RootView: View {
    @ObservedObject var store: FanStore
    var scrollable = true

    var body: some View {
        VStack(spacing: 0) {
            TabBar(store: store)
            Divider().overlay(Palette.stroke)
            Group {
                switch store.tab {
                case .overview: OverviewPage(store: store, scrollable: scrollable)
                case .curve: CurvePage(store: store, scrollable: scrollable)
                case .settings: SettingsPage(store: store, scrollable: scrollable)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Palette.panel)
        .overlay(alignment: .bottomTrailing) { Toasts(store: store) }
    }
}

struct TabBar: View {
    @ObservedObject var store: FanStore

    var body: some View {
        HStack(spacing: 4) {
            ForEach(FanStore.Tab.allCases) { tab in
                Button {
                    store.tab = tab
                } label: {
                    HStack(spacing: 5) {
                        Image(systemName: tab.symbol).font(.system(size: 11))
                        Text(tab.label).font(.system(size: 12, weight: .medium))
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(
                        RoundedRectangle(cornerRadius: 7)
                            .fill(store.tab == tab ? Palette.raised : .clear))
                    .foregroundStyle(store.tab == tab ? Color.white : Palette.muted)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }

            Spacer(minLength: 8)

            HStack(spacing: 6) {
                Circle()
                    .fill(store.connected ? Palette.ok : Palette.danger)
                    .frame(width: 6, height: 6)
                Text(store.connected ? "已连接" : "未连接")
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.muted)
                if store.draftDirty {
                    Badge(text: "未保存", colour: Palette.warn)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Palette.panel)
    }
}

struct Toasts: View {
    @ObservedObject var store: FanStore

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            ForEach(store.toasts) { toast in
                Text(toast.text)
                    .font(.system(size: 12))
                    .foregroundStyle(toast.isError ? Palette.danger : Palette.ok)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(
                        RoundedRectangle(cornerRadius: 8)
                            .fill(Color(red: 0x24 / 255, green: 0x28 / 255, blue: 0x2E / 255))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .strokeBorder(
                                (toast.isError ? Palette.danger : Palette.ok).opacity(0.6),
                                lineWidth: 1)
                    )
            }
        }
        .padding(16)
        .animation(.easeInOut(duration: 0.15), value: store.toasts.count)
    }
}

/// Wraps page content; `ScrollView` for the real window, a plain stack when
/// rendering offscreen with `ImageRenderer`.
struct PageScaffold<Content: View>: View {
    var scrollable: Bool
    @ViewBuilder var content: Content

    var body: some View {
        if scrollable {
            ScrollView { content }
        } else {
            content
        }
    }
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
