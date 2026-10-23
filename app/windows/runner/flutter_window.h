#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/encodable_value.h>
#include <flutter/flutter_view_controller.h>
#include <flutter/method_channel.h>

#include <memory>
#include <string>
#include <vector>

#include "win32_window.h"

// WM_COPYDATA from a later launch: its command line (UTF-16), for the
// running Nudge to act on. See main.cpp.
constexpr ULONG_PTR kLaunchMessage = 0x4E554447;  // "NUDG"

// A window that does nothing but host a Flutter view.
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|.
  explicit FlutterWindow(const flutter::DartProject& project);
  virtual ~FlutterWindow();

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // Tells Dart about each copy ("nudge/clipboard"), for clipboard sync.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> clipboard_;

  // Reads what was just copied and passes it on, unless it's marked private.
  void ReportCopy();

  // Tells Dart about later launches ("nudge/instance"): their arguments, so
  // files sent from Explorer arrive and the window comes back.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> instance_;

  // Launches that came before Dart was listening.
  std::vector<flutter::EncodableValue> pending_launches_;
  bool dart_listening_ = false;

  // A later launch's command line arrived.
  void OnLaunch(const std::wstring& command_line);
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
