#ifndef RUNNER_CLIPBOARD_IMAGE_H_
#define RUNNER_CLIPBOARD_IMAGE_H_

#include <windows.h>

#include <cstdint>
#include <vector>

// Copied images, for clipboard sync. PNG on the wire; the clipboard gets
// both a bitmap (CF_DIB, which every app pastes) and "PNG" (keeps
// transparency in apps that know it).

// The image on the clipboard as PNG, or empty if there's none. The
// clipboard must already be open.
std::vector<uint8_t> ReadClipboardImagePng();

// Put an image (PNG, JPEG, GIF, BMP...) on the clipboard, replacing what's
// there. Returns whether it worked.
bool WriteClipboardImage(HWND owner, const std::vector<uint8_t>& image);

#endif  // RUNNER_CLIPBOARD_IMAGE_H_
