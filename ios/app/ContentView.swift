import SwiftUI

struct ContentView: View {
    @StateObject private var vm = PortalViewModel()
    @State private var showLogoffConfirm = false

    var body: some View {
        NavigationStack {
            Form {
                Section("账号") {
                    TextField("学号", text: $vm.account)
                        .textContentType(.username)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    SecureField("密码", text: $vm.password)
                        .textContentType(.password)
                    Picker("运营商", selection: $vm.selectedOperator) {
                        ForEach(PortalViewModel.Operator.allCases) { op in
                            Text(op.title).tag(op)
                        }
                    }
                }

                Section("状态") {
                    Text(vm.statusText)
                        .font(.system(.body, design: .monospaced))
                    if let err = vm.lastError {
                        Text(err).foregroundStyle(.red).font(.footnote)
                    }
                }

                Section {
                    Button {
                        Task { await vm.login() }
                    } label: {
                        Label("立即登录", systemImage: "wifi")
                    }
                    .disabled(vm.isBusy || vm.account.isEmpty || vm.password.isEmpty)

                    Button {
                        Task { await vm.refreshStatus() }
                    } label: {
                        Label("刷新状态", systemImage: "arrow.clockwise")
                    }
                    .disabled(vm.isBusy)

                    Button(role: .destructive) {
                        showLogoffConfirm = true
                    } label: {
                        Label("断开连接", systemImage: "wifi.slash")
                    }
                    .disabled(vm.isBusy)
                }

                Section {
                    Toggle("自动登录（后台尽力而为）", isOn: Binding(
                        get: { vm.autoLoginEnabled },
                        set: { vm.setAutoLogin($0) }
                    ))
                } footer: {
                    Text("iOS 不允许后台常驻，系统按自己的节奏调度，无法保证及时性。详见 ios/app-plan.md。")
                }
            }
            .navigationTitle("校园网助手")
            .overlay {
                if vm.isBusy { ProgressView() }
            }
            .confirmationDialog("确认断开校园网认证？", isPresented: $showLogoffConfirm, titleVisibility: .visible) {
                Button("确认断开", role: .destructive) {
                    Task { await vm.logoff() }
                }
                Button("取消", role: .cancel) {}
            }
        }
    }
}

#Preview {
    ContentView()
}
