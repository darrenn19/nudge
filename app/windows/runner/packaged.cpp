#include "packaged.h"

#include <windows.h>
#include <appmodel.h>

#include <thread>

#include <winrt/Windows.ApplicationModel.Activation.h>
#include <winrt/Windows.ApplicationModel.DataTransfer.ShareTarget.h>
#include <winrt/Windows.ApplicationModel.DataTransfer.h>
#include <winrt/Windows.ApplicationModel.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Storage.h>

#pragma comment(lib, "windowsapp")

namespace {

using namespace winrt::Windows::ApplicationModel;

// The task's ID in AppxManifest.xml.
constexpr wchar_t kStartupTask[] = L"NudgeStartup";

// Run |work| on a thread of its own in the multithreaded apartment: WinRT's
// blocking waits aren't allowed on the UI thread's apartment.
template <typename T, typename F>
T InBackground(F work) {
  T result{};
  std::thread([&] {
    winrt::init_apartment(winrt::apartment_type::multi_threaded);
    try {
      result = work();
    } catch (...) {
      // Leave the default: not packaged, or the API isn't there.
    }
    winrt::uninit_apartment();
  }).join();
  return result;
}

}  // namespace

bool IsPackaged() {
  UINT32 length = 0;
  return GetCurrentPackageFullName(&length, nullptr) != APPMODEL_ERROR_NO_PACKAGE;
}

PackagedLaunch GetPackagedLaunch() {
  if (!IsPackaged()) return {};
  return InBackground<PackagedLaunch>([] {
    PackagedLaunch launch;
    auto args = AppInstance::GetActivatedEventArgs();
    if (!args) return launch;
    switch (args.Kind()) {
      case Activation::ActivationKind::StartupTask:
        launch.at_login = true;
        break;
      case Activation::ActivationKind::ShareTarget: {
        auto share = args.as<Activation::ShareTargetActivatedEventArgs>().ShareOperation();
        auto data = share.Data();
        using DataTransfer::StandardDataFormats;
        if (data.Contains(StandardDataFormats::StorageItems())) {
          for (const auto& item : data.GetStorageItemsAsync().get()) {
            launch.shared.push_back(std::wstring(item.Path()));
          }
        } else if (data.Contains(StandardDataFormats::WebLink())) {
          launch.text = std::wstring(data.GetWebLinkAsync().get().ToString());
        } else if (data.Contains(StandardDataFormats::Text())) {
          launch.text = std::wstring(data.GetTextAsync().get());
        }
        // Nudge takes it from here; the Share panel can close.
        share.ReportCompleted();
        break;
      }
      default:
        break;
    }
    return launch;
  });
}

std::optional<bool> StartupTaskEnabled() {
  if (!IsPackaged()) return std::nullopt;
  return InBackground<std::optional<bool>>([]() -> std::optional<bool> {
    auto task = StartupTask::GetAsync(kStartupTask).get();
    auto state = task.State();
    return state == StartupTaskState::Enabled || state == StartupTaskState::EnabledByPolicy;
  });
}

std::optional<bool> SetStartupTask(bool on) {
  if (!IsPackaged()) return std::nullopt;
  return InBackground<std::optional<bool>>([on]() -> std::optional<bool> {
    auto task = StartupTask::GetAsync(kStartupTask).get();
    if (on) {
      auto state = task.RequestEnableAsync().get();
      return state == StartupTaskState::Enabled || state == StartupTaskState::EnabledByPolicy;
    }
    task.Disable();
    return false;
  });
}
