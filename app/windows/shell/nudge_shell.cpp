// "Send with Nudge" in Windows 11's top-level right-click menu.
//
// Windows 11 shows packaged apps' IExplorerCommand handlers in the new menu
// (older registry verbs only appear under "Show more options"). The MSIX
// manifest (windows/packaging/AppxManifest.xml) registers this class; when
// chosen, it starts nudge.exe (next to this DLL) with `--send <paths>`, and
// the running Nudge takes them (see runner/main.cpp).

#include <windows.h>
#include <shlobj.h>
#include <shobjidl_core.h>
#include <shlwapi.h>

#include <atomic>
#include <new>
#include <string>

#pragma comment(lib, "shlwapi.lib")

namespace {

// {7ACAB74A-1F6D-4CFD-9F82-A5659C26F25F}: also in AppxManifest.xml.
constexpr CLSID kSendWithNudge = {
    0x7acab74a, 0x1f6d, 0x4cfd, {0x9f, 0x82, 0xa5, 0x65, 0x9c, 0x26, 0xf2, 0x5f}};

HMODULE g_module = nullptr;
std::atomic<long> g_objects{0};

// nudge.exe, beside this DLL.
std::wstring NudgeExe() {
  wchar_t path[MAX_PATH];
  if (!GetModuleFileNameW(g_module, path, MAX_PATH)) return L"";
  PathRemoveFileSpecW(path);
  return std::wstring(path) + L"\\nudge.exe";
}

// Quote an argument for CreateProcess (paths can hold spaces, not quotes).
std::wstring Quoted(const std::wstring& s) { return L"\"" + s + L"\""; }

class SendWithNudge : public IExplorerCommand {
 public:
  SendWithNudge() { ++g_objects; }
  virtual ~SendWithNudge() { --g_objects; }

  // IUnknown
  IFACEMETHODIMP QueryInterface(REFIID riid, void** out) override {
    if (!out) return E_POINTER;
    if (riid == IID_IUnknown || riid == IID_IExplorerCommand) {
      *out = static_cast<IExplorerCommand*>(this);
      AddRef();
      return S_OK;
    }
    *out = nullptr;
    return E_NOINTERFACE;
  }
  IFACEMETHODIMP_(ULONG) AddRef() override { return ++refs_; }
  IFACEMETHODIMP_(ULONG) Release() override {
    long left = --refs_;
    if (left == 0) delete this;
    return left;
  }

  // IExplorerCommand
  IFACEMETHODIMP GetTitle(IShellItemArray*, LPWSTR* title) override {
    return SHStrDupW(L"Send with Nudge", title);
  }
  IFACEMETHODIMP GetIcon(IShellItemArray*, LPWSTR* icon) override {
    return SHStrDupW((NudgeExe() + L",0").c_str(), icon);
  }
  IFACEMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* tip) override {
    *tip = nullptr;
    return E_NOTIMPL;
  }
  IFACEMETHODIMP GetCanonicalName(GUID* name) override {
    *name = kSendWithNudge;
    return S_OK;
  }
  IFACEMETHODIMP GetState(IShellItemArray*, BOOL, EXPCMDSTATE* state) override {
    *state = ECS_ENABLED;
    return S_OK;
  }
  IFACEMETHODIMP GetFlags(EXPCMDFLAGS* flags) override {
    *flags = ECF_DEFAULT;
    return S_OK;
  }
  IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** commands) override {
    *commands = nullptr;
    return E_NOTIMPL;
  }

  IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) override {
    if (!items) return S_OK;
    DWORD count = 0;
    items->GetCount(&count);
    std::wstring command = Quoted(NudgeExe()) + L" --send";
    for (DWORD i = 0; i < count; i++) {
      IShellItem* item = nullptr;
      if (FAILED(items->GetItemAt(i, &item))) continue;
      PWSTR path = nullptr;
      if (SUCCEEDED(item->GetDisplayName(SIGDN_FILESYSPATH, &path))) {
        command += L" " + Quoted(path);
        CoTaskMemFree(path);
      }
      item->Release();
    }
    STARTUPINFOW startup = {sizeof(startup)};
    PROCESS_INFORMATION process = {};
    if (CreateProcessW(nullptr, command.data(), nullptr, nullptr, FALSE, 0, nullptr,
                       nullptr, &startup, &process)) {
      // Let it bring Nudge's window to the front.
      AllowSetForegroundWindow(process.dwProcessId);
      CloseHandle(process.hThread);
      CloseHandle(process.hProcess);
    }
    return S_OK;
  }

 private:
  std::atomic<long> refs_{1};
};

class Factory : public IClassFactory {
 public:
  IFACEMETHODIMP QueryInterface(REFIID riid, void** out) override {
    if (!out) return E_POINTER;
    if (riid == IID_IUnknown || riid == IID_IClassFactory) {
      *out = static_cast<IClassFactory*>(this);
      AddRef();
      return S_OK;
    }
    *out = nullptr;
    return E_NOINTERFACE;
  }
  IFACEMETHODIMP_(ULONG) AddRef() override { return 2; }   // static
  IFACEMETHODIMP_(ULONG) Release() override { return 1; }  // static
  IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** out) override {
    if (outer) return CLASS_E_NOAGGREGATION;
    auto* command = new (std::nothrow) SendWithNudge();
    if (!command) return E_OUTOFMEMORY;
    HRESULT result = command->QueryInterface(riid, out);
    command->Release();
    return result;
  }
  IFACEMETHODIMP LockServer(BOOL lock) override {
    lock ? ++g_objects : --g_objects;
    return S_OK;
  }
};

Factory g_factory;

}  // namespace

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
  if (reason == DLL_PROCESS_ATTACH) {
    g_module = module;
    DisableThreadLibraryCalls(module);
  }
  return TRUE;
}

STDAPI DllGetClassObject(REFCLSID clsid, REFIID riid, LPVOID* out) {
  if (clsid != kSendWithNudge) return CLASS_E_CLASSNOTAVAILABLE;
  return g_factory.QueryInterface(riid, out);
}

STDAPI DllCanUnloadNow() { return g_objects == 0 ? S_OK : S_FALSE; }
