import SwiftUI
import UserNotifications

@main
struct SNNUAutoLoginApp: App {

    @Environment(\.scenePhase) private var scenePhase

    init() {
        // 必须在启动完成前注册，否则 BGTaskScheduler 会报错
        BackgroundRefresh.register()
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .onChange(of: scenePhase) { phase in
            if phase == .background {
                BackgroundRefresh.scheduleNext()
            }
        }
    }
}
