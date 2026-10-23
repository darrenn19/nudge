#include "flutter_window.h"

#include <cwchar>
#include <optional>

#include <flutter/standard_method_codec.h>

#include "flutter/generated_plugin_registrant.h"
#include "utils.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

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

  // Later launches. Dart says "listen" once it's ready, and gets any that
  // came before.
  instance_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "nudge/instance",
      &flutter::StandardMethodCodec::GetInstance());
  instance_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
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
  if (!wcsstr(GetCommandLineW(), L" --background")) {
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
  clipboard_ = nullptr;
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
