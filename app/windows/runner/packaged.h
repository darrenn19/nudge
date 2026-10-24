#ifndef RUNNER_PACKAGED_H_
#define RUNNER_PACKAGED_H_

#include <optional>
#include <string>
#include <vector>

// When Nudge is installed as an MSIX package (windows/packaging), Windows
// starts it in ways an exe on its own isn't: at login through the package's
// startup task, and from the Share panel. See packaged.cpp.

// Whether this process runs as part of an installed package.
bool IsPackaged();

// How a packaged launch came about.
struct PackagedLaunch {
  // Started at login: stay in the tray.
  bool at_login = false;
  // Files shared to Nudge from the Share panel.
  std::vector<std::wstring> shared;
  // Text or a link shared from the Share panel: for the devices' clipboards.
  std::wstring text;
};

// What started this process (packaged only; empty otherwise).
PackagedLaunch GetPackagedLaunch();

// Whether the package's "start at login" task is on; nullopt when not
// packaged or not available.
std::optional<bool> StartupTaskEnabled();

// Turn it on or off. Returns whether it's on now (Windows may refuse, if
// the user turned it off in Task Manager).
std::optional<bool> SetStartupTask(bool on);

#endif  // RUNNER_PACKAGED_H_
