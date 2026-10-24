#include "clipboard_image.h"

#include <objidl.h>
#include <shlwapi.h>

// GDI+ uses min/max from windows.h, which NOMINMAX turns off.
#include <algorithm>
namespace Gdiplus {
using std::max;
using std::min;
}  // namespace Gdiplus
#include <gdiplus.h>

#pragma comment(lib, "gdiplus.lib")
#pragma comment(lib, "shlwapi.lib")

namespace {

// Biggest image sent or accepted (as the core allows).
constexpr size_t kMaxImage = 48u << 20;

UINT PngFormat() {
  static const UINT format = RegisterClipboardFormatW(L"PNG");
  return format;
}

// GDI+ is started once, on first use, and left running.
bool StartGdiplus() {
  static bool started = [] {
    Gdiplus::GdiplusStartupInput input;
    ULONG_PTR token = 0;
    return Gdiplus::GdiplusStartup(&token, &input, nullptr) == Gdiplus::Ok;
  }();
  return started;
}

bool PngEncoder(CLSID* clsid) {
  UINT count = 0, size = 0;
  if (Gdiplus::GetImageEncodersSize(&count, &size) != Gdiplus::Ok || size == 0) return false;
  std::vector<uint8_t> buffer(size);
  auto* encoders = reinterpret_cast<Gdiplus::ImageCodecInfo*>(buffer.data());
  if (Gdiplus::GetImageEncoders(count, size, encoders) != Gdiplus::Ok) return false;
  for (UINT i = 0; i < count; i++) {
    if (wcscmp(encoders[i].MimeType, L"image/png") == 0) {
      *clsid = encoders[i].Clsid;
      return true;
    }
  }
  return false;
}

std::vector<uint8_t> EncodePng(Gdiplus::Bitmap& bitmap) {
  CLSID png;
  if (!PngEncoder(&png)) return {};
  IStream* stream = SHCreateMemStream(nullptr, 0);
  if (!stream) return {};
  std::vector<uint8_t> out;
  if (bitmap.Save(stream, &png, nullptr) == Gdiplus::Ok) {
    STATSTG stat{};
    if (SUCCEEDED(stream->Stat(&stat, STATFLAG_NONAME)) && stat.cbSize.QuadPart <= kMaxImage) {
      out.resize(static_cast<size_t>(stat.cbSize.QuadPart));
      LARGE_INTEGER zero{};
      stream->Seek(zero, STREAM_SEEK_SET, nullptr);
      ULONG read = 0;
      stream->Read(out.data(), static_cast<ULONG>(out.size()), &read);
      out.resize(read);
    }
  }
  stream->Release();
  return out;
}

// A CF_DIB as PNG.
std::vector<uint8_t> DibToPng(HANDLE dib) {
  auto* info = static_cast<BITMAPINFO*>(GlobalLock(dib));
  if (!info) return {};
  std::vector<uint8_t> out;
  const BITMAPINFOHEADER& h = info->bmiHeader;
  // Pixels follow the header, the colour table and (BI_BITFIELDS with the
  // basic header) three masks.
  size_t colors = h.biClrUsed ? h.biClrUsed : (h.biBitCount <= 8 ? (1u << h.biBitCount) : 0);
  size_t offset = h.biSize + colors * sizeof(RGBQUAD);
  if (h.biCompression == BI_BITFIELDS && h.biSize == sizeof(BITMAPINFOHEADER)) {
    offset += 3 * sizeof(DWORD);
  }
  const auto* bits = reinterpret_cast<const uint8_t*>(info) + offset;
  // 32-bit bitmaps may carry transparency GDI+ would drop: copy by hand.
  if (h.biBitCount == 32 && (h.biCompression == BI_RGB || h.biCompression == BI_BITFIELDS)) {
    int width = h.biWidth, height = std::abs(h.biHeight);
    bool bottom_up = h.biHeight > 0;
    Gdiplus::Bitmap bitmap(width, height, PixelFormat32bppARGB);
    Gdiplus::BitmapData data;
    Gdiplus::Rect rect(0, 0, width, height);
    if (bitmap.LockBits(&rect, Gdiplus::ImageLockModeWrite, PixelFormat32bppARGB, &data) ==
        Gdiplus::Ok) {
      // Many apps leave alpha at 0 in 32-bit bitmaps: then it means opaque.
      bool any_alpha = false;
      for (int y = 0; y < height && !any_alpha; y++) {
        const uint8_t* row = bits + static_cast<size_t>(y) * width * 4;
        for (int x = 0; x < width; x++) {
          if (row[x * 4 + 3] != 0) {
            any_alpha = true;
            break;
          }
        }
      }
      for (int y = 0; y < height; y++) {
        const uint8_t* src = bits + static_cast<size_t>(bottom_up ? height - 1 - y : y) * width * 4;
        auto* dst = static_cast<uint8_t*>(data.Scan0) + static_cast<ptrdiff_t>(y) * data.Stride;
        memcpy(dst, src, static_cast<size_t>(width) * 4);
        if (!any_alpha) {
          for (int x = 0; x < width; x++) dst[x * 4 + 3] = 255;
        }
      }
      bitmap.UnlockBits(&data);
      out = EncodePng(bitmap);
    }
  } else {
    Gdiplus::Bitmap bitmap(info, const_cast<uint8_t*>(bits));
    if (bitmap.GetLastStatus() == Gdiplus::Ok) out = EncodePng(bitmap);
  }
  GlobalUnlock(dib);
  return out;
}

HGLOBAL ToGlobal(const void* bytes, size_t size) {
  HGLOBAL global = GlobalAlloc(GMEM_MOVEABLE, size);
  if (!global) return nullptr;
  void* target = GlobalLock(global);
  if (!target) {
    GlobalFree(global);
    return nullptr;
  }
  memcpy(target, bytes, size);
  GlobalUnlock(global);
  return global;
}

// The bitmap as a 32-bit CF_DIB, bottom-up as most apps expect.
HGLOBAL ToDib(Gdiplus::Bitmap& bitmap) {
  int width = static_cast<int>(bitmap.GetWidth()), height = static_cast<int>(bitmap.GetHeight());
  Gdiplus::BitmapData data;
  Gdiplus::Rect rect(0, 0, width, height);
  if (bitmap.LockBits(&rect, Gdiplus::ImageLockModeRead, PixelFormat32bppARGB, &data) !=
      Gdiplus::Ok) {
    return nullptr;
  }
  size_t row = static_cast<size_t>(width) * 4;
  std::vector<uint8_t> dib(sizeof(BITMAPINFOHEADER) + row * height);
  auto* header = reinterpret_cast<BITMAPINFOHEADER*>(dib.data());
  header->biSize = sizeof(BITMAPINFOHEADER);
  header->biWidth = width;
  header->biHeight = height;  // bottom-up
  header->biPlanes = 1;
  header->biBitCount = 32;
  header->biCompression = BI_RGB;
  header->biSizeImage = static_cast<DWORD>(row * height);
  for (int y = 0; y < height; y++) {
    const auto* src = static_cast<const uint8_t*>(data.Scan0) + static_cast<ptrdiff_t>(y) * data.Stride;
    memcpy(dib.data() + sizeof(BITMAPINFOHEADER) + row * (height - 1 - y), src, row);
  }
  bitmap.UnlockBits(&data);
  return ToGlobal(dib.data(), dib.size());
}

}  // namespace

std::vector<uint8_t> ReadClipboardImagePng() {
  // Already PNG (browsers, Snipping Tool): as is.
  if (IsClipboardFormatAvailable(PngFormat())) {
    HANDLE data = GetClipboardData(PngFormat());
    if (data) {
      size_t size = GlobalSize(data);
      const void* bytes = GlobalLock(data);
      std::vector<uint8_t> out;
      if (bytes && size > 0 && size <= kMaxImage) {
        out.assign(static_cast<const uint8_t*>(bytes), static_cast<const uint8_t*>(bytes) + size);
      }
      if (bytes) GlobalUnlock(data);
      if (!out.empty()) return out;
    }
  }
  if (!IsClipboardFormatAvailable(CF_DIB) || !StartGdiplus()) return {};
  HANDLE dib = GetClipboardData(CF_DIB);
  return dib ? DibToPng(dib) : std::vector<uint8_t>{};
}

bool WriteClipboardImage(HWND owner, const std::vector<uint8_t>& image) {
  if (image.empty() || image.size() > kMaxImage || !StartGdiplus()) return false;
  IStream* stream = SHCreateMemStream(image.data(), static_cast<UINT>(image.size()));
  if (!stream) return false;
  bool ok = false;
  {
    Gdiplus::Bitmap bitmap(stream);
    if (bitmap.GetLastStatus() == Gdiplus::Ok) {
      HGLOBAL dib = ToDib(bitmap);
      std::vector<uint8_t> png = EncodePng(bitmap);
      HGLOBAL png_global = png.empty() ? nullptr : ToGlobal(png.data(), png.size());
      bool opened = false;
      for (int i = 0; i < 5 && !opened; i++) {
        opened = OpenClipboard(owner);
        if (!opened) Sleep(20);
      }
      if (opened && dib) {
        EmptyClipboard();
        ok = SetClipboardData(CF_DIB, dib) != nullptr;
        if (ok) dib = nullptr;  // the clipboard owns it now
        if (png_global && SetClipboardData(PngFormat(), png_global)) png_global = nullptr;
      }
      if (opened) CloseClipboard();
      if (dib) GlobalFree(dib);
      if (png_global) GlobalFree(png_global);
    }
  }
  stream->Release();
  return ok;
}
