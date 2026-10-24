#include "flutter_window.h"

#include <cwchar>
#include <optional>

#include <flutter/standard_method_codec.h>
#include <knownfolders.h>
#include <shlobj.h>
#include <wtsapi32.h>

#pragma comment(lib, "wtsapi32.lib")

#include "flutter/generated_plugin_registrant.h"
#include "packaged.h"
#include "utils.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project, bool hidden)
    : project_(project), hidden_(hidden) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  // Clipboard sync: hear about every copy, even with the window hidden in
  // the tray (it still exists, so it still gets messages).
  clipboard_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "nudge/clipboard",
      &flutter::StandardMethodCodec::GetInstance());
  AddClipboardFormatListener(GetHandle());

  // Locking and unlocking (Win+L), even with the window hidden.
  session_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "nudge/session",
      &flutter::StandardMethodCodec::GetInstance());
  WTSRegisterSessionNotification(GetHandle(), NOTIFY_FOR_THIS_SESSION);

  // Later launches. Dart says "listen" once it's ready, and gets any that
  // came before.
  instance_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "nudge/instance",
      &flutter::StandardMethodCodec::GetInstance());
  instance_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
        const std::string& method = call.method_name();
        if (method == "installSendTo") {
          result->Success(flutter::EncodableValue(InstallSendTo()));
          return;
        }
        // Installed as a package: start at login through its startup task.
        if (method == "packaged") {
          result->Success(flutter::EncodableValue(IsPackaged()));
          return;
        }
        if (method == "startupTask" || method == "setStartupTask") {
          auto* on = std::get_if<bool>(call.arguments());
          std::optional<bool> state =
              method == "startupTask" ? StartupTaskEnabled() : SetStartupTask(on && *on);
          result->Success(state ? flutter::EncodableValue(*state) : flutter::EncodableValue());
          return;
        }
        if (call.method_name() != "listen") {
          result->NotImplemented();
          return;
        }
        dart_listening_ = true;
        result->Success(flutter::EncodableValue(flutter::EncodableList(
            pending_launches_.begin(), pending_launches_.end())));
        pending_launches_.clear();
      });

  // Started at login (--background): stay in the tray until opened. The
  // Dart side shows the window when the user asks for it.
  if (!hidden_) {
    flutter_controller_->engine()->SetNextFrameCallback([&]() {
      this->Show();
    });
  }

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  RemoveClipboardFormatListener(GetHandle());
  WTSUnRegisterSessionNotification(GetHandle());
  clipboard_ = nullptr;
  session_ = nullptr;
  instance_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
    case WM_CLIPBOARDUPDATE:
      ReportCopy();
      return 0;
    case WM_WTSSESSION_CHANGE:
      if (session_ && (wparam == WTS_SESSION_LOCK || wparam == WTS_SESSION_UNLOCK)) {
        session_->InvokeMethod(
            "locked", std::make_unique<flutter::EncodableValue>(wparam == WTS_SESSION_LOCK));
      }
      break;
    case WM_COPYDATA: {
      auto* data = reinterpret_cast<const COPYDATASTRUCT*>(lparam);
      if (data && data->dwData == kLaunchMessage && data->lpData &&
          data->cbData >= sizeof(wchar_t)) {
        std::wstring line(static_cast<const wchar_t*>(data->lpData),
                          data->cbData / sizeof(wchar_t) - 1);
        OnLaunch(line);
        return TRUE;
      }
      break;
    }
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}

void FlutterWindow::OnLaunch(const std::wstring& command_line) {
  flutter::EncodableList args;
  for (const auto& arg : ArgumentsOf(command_line.c_str())) {
    args.push_back(flutter::EncodableValue(arg));
  }
  flutter::EncodableValue launch(std::move(args));
  if (dart_listening_ && instance_) {
    instance_->InvokeMethod("launched", std::make_unique<flutter::EncodableValue>(launch));
  } else {
    pending_launches_.push_back(std::move(launch));
  }
}

bool FlutterWindow::InstallSendTo() {
  wchar_t exe[MAX_PATH];
  if (!GetModuleFileNameW(nullptr, exe, MAX_PATH)) return false;
  PWSTR folder = nullptr;
  if (FAILED(SHGetKnownFolderPath(FOLDERID_SendTo, 0, nullptr, &folder))) return false;
  std::wstring path = std::wstring(folder) + L"\\Nudge.lnk";
  CoTaskMemFree(folder);

  IShellLinkW* link = nullptr;
  if (FAILED(CoCreateInstance(CLSID_ShellLink, nullptr, CLSCTX_INPROC_SERVER,
                              IID_PPV_ARGS(&link)))) {
    return false;
  }
  // Explorer adds the selected files after these arguments, all at once.
  link->SetPath(exe);
  link->SetArguments(L"--send");
  link->SetIconLocation(exe, 0);
  link->SetDescription(L"Send with Nudge");
  IPersistFile* file = nullptr;
  bool saved = SUCCEEDED(link->QueryInterface(IID_PPV_ARGS(&file))) &&
               SUCCEEDED(file->Save(path.c_str(), TRUE));
  if (file) file->Release();
  link->Release();
  return saved;
}

namespace {

// Whether the clipboard carries a DWORD format set to 0: how password
// managers say "don't keep or sync this".
bool FormatSaysNo(UINT format) {
  if (!IsClipboardFormatAvailable(format)) return false;
  HANDLE data = GetClipboardData(format);
  if (!data) return false;
  auto* value = static_cast<DWORD*>(GlobalLock(data));
  bool no = value && *value == 0;
  GlobalUnlock(data);
  return no;
}

}  // namespace

void FlutterWindow::ReportCopy() {
  if (!clipboard_) return;
  static const UINT exclude =
      RegisterClipboardFormatW(L"ExcludeClipboardContentFromMonitorProcessing");
  static const UINT history = RegisterClipboardFormatW(L"CanIncludeInClipboardHistory");
  static const UINT cloud = RegisterClipboardFormatW(L"CanUploadToCloudClipboard");

  // Another app may hold the clipboard for a moment.
  bool opened = false;
  for (int i = 0; i < 5 && !opened; i++) {
    opened = OpenClipboard(GetHandle());
    if (!opened) Sleep(20);
  }
  if (!opened) return;
  bool keep_private = IsClipboardFormatAvailable(exclude) || FormatSaysNo(history) ||
                      FormatSaysNo(cloud);
  std::wstring text;
  if (!keep_private && IsClipboardFormatAvailable(CF_UNICODETEXT)) {
    HANDLE data = GetClipboardData(CF_UNICODETEXT);
    if (data) {
      auto* chars = static_cast<const wchar_t*>(GlobalLock(data));
      if (chars) text = chars;
      GlobalUnlock(data);
    }
  }
  CloseClipboard();
  // Text only, and nothing huge (the other side takes up to 1 MB).
  if (keep_private || text.empty() || text.size() > (1 << 18)) return;
  clipboard_->InvokeMethod("copied",
                           std::make_unique<flutter::EncodableValue>(Utf8FromUtf16(text.c_str())));
}
