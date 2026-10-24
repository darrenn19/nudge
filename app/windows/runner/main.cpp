#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <string>

#include "flutter_window.h"
#include "packaged.h"
#include "utils.h"

namespace {

// One Nudge at a time. A later launch (the Start menu, Explorer's "Send
// with Nudge", the Share panel) hands its command line to the running Nudge
// and exits, before any of Flutter loads.
void HandOver(std::wstring line) {
  // Started at login while Nudge is already running: nothing to say.
  if (line.find(L" --background") != std::wstring::npos) return;
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

// |text| in a new temporary file (UTF-8); its path, or empty.
std::wstring SaveText(const std::wstring& text) {
  wchar_t dir[MAX_PATH], path[MAX_PATH];
  if (!GetTempPathW(MAX_PATH, dir) || !GetTempFileNameW(dir, L"ndg", 0, path)) return L"";
  std::string utf8 = Utf8FromUtf16(text.c_str());
  HANDLE file = CreateFileW(path, GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                            FILE_ATTRIBUTE_TEMPORARY, nullptr);
  if (file == INVALID_HANDLE_VALUE) return L"";
  DWORD written = 0;
  WriteFile(file, utf8.data(), static_cast<DWORD>(utf8.size()), &written, nullptr);
  CloseHandle(file);
  return path;
}

// This launch as a command line: the real one, or, for a packaged launch
// from the Share panel or at login, what that amounts to.
std::wstring EffectiveCommandLine() {
  std::wstring line = GetCommandLineW();
  PackagedLaunch launch = GetPackagedLaunch();
  if (launch.at_login && line.find(L" --background") == std::wstring::npos) {
    line += L" --background";
  }
  wchar_t exe[MAX_PATH];
  GetModuleFileNameW(nullptr, exe, MAX_PATH);
  if (!launch.shared.empty()) {
    line = std::wstring(L"\"") + exe + L"\" --send";
    for (const auto& path : launch.shared) line += L" \"" + path + L"\"";
  } else if (!launch.text.empty()) {
    // Text can hold anything a command line can't: it goes in a file, which
    // the app reads and deletes.
    std::wstring file = SaveText(launch.text);
    if (!file.empty()) line = std::wstring(L"\"") + exe + L"\" --clip \"" + file + L"\"";
  }
  return line;
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  std::wstring line = EffectiveCommandLine();

  // Held until exit: owning it is what makes this the running Nudge.
  HANDLE single = CreateMutexW(nullptr, TRUE, L"Local\\Nudge.SingleInstance");
  if (single && GetLastError() == ERROR_ALREADY_EXISTS) {
    HandOver(line);
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

  project.set_dart_entrypoint_arguments(ArgumentsOf(line.c_str()));

  // Started at login (--background): stay in the tray until opened.
  bool hidden = line.find(L" --background") != std::wstring::npos;
  FlutterWindow window(project, hidden);
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
