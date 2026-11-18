import SwiftUI
import TomiDesign

@main
struct TomiApp: App {
    @UIApplicationDelegateAdaptor(TomiAppDelegate.self) private var appDelegate
    @State private var container = AppContainer()
    @Environment(\.scenePhase) private var scenePhase

    init() {
        TomiFonts.register()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(container)
                .task { await container.start() }
                .onOpenURL { container.open($0) }
        }
        .onChange(of: scenePhase) { _, phase in
            container.scenePhaseChanged(phase)
        }
    }
}
