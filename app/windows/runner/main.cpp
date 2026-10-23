#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <string>

#include "flutter_window.h"
#include "utils.h"

namespace {

// One Nudge at a time. A later launch (the Start menu, or Explorer's "Send
// with Nudge", which starts one process per selected file) hands its command
// line to the running Nudge and exits, before any of Flutter loads.
void HandOver(const wchar_t* command_line) {
  // Started at login while Nudge is already running: nothing to say.
  if (wcsstr(command_line, L"--background")) return;
  std::wstring line = GetCommandLineW();
  COPYDATASTRUCT data = {};
  data.dwData = kLaunchMessage;
  data.cbData = static_cast<DWORD>((line.size() + 1) * sizeof(wchar_t));
  data.lpData = line.data();
  // The running one may still be starting: give it up to ten seconds.
  for (int i = 0; i < 100; i++) {
    HWND running = FindWindowW(kWindowClassName, nullptr);
    if (running) {
      // Let it bring its window to the front.
      DWORD pid = 0;
      GetWindowThreadProcessId(running, &pid);
      AllowSetForegroundWindow(pid);
      DWORD_PTR result = 0;
      if (SendMessageTimeoutW(running, WM_COPYDATA, 0,
                              reinterpret_cast<LPARAM>(&data), SMTO_ABORTIFHUNG,
                              5000, &result) &&
          result) {
        return;
      }
    }
    Sleep(100);
  }
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // Held until exit: owning it is what makes this the running Nudge.
  HANDLE single = CreateMutexW(nullptr, TRUE, L"Local\\Nudge.SingleInstance");
  if (single && GetLastError() == ERROR_ALREADY_EXISTS) {
    HandOver(command_line);
    CloseHandle(single);
    return EXIT_SUCCESS;
  }

  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");
  // Skia, not Impeller: measured on Windows, Impeller took ~1.5 s longer to
  // start and held ~700 MB more memory, for no visible difference here.
  project.set_impeller_switch(flutter::ImpellerSwitch::Disabled);

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  if (!window.Create(L"Nudge", Win32Window::Size(480, 720))) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  if (single) CloseHandle(single);
  return EXIT_SUCCESS;
}
