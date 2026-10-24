# "Send with Nudge" in Windows 11's top-level right-click menu (packaged
# builds only; see nudge_shell.cpp).
cmake_minimum_required(VERSION 3.14)
add_library(nudge_shell SHARED nudge_shell.cpp nudge_shell.def)
target_compile_definitions(nudge_shell PRIVATE UNICODE _UNICODE)
target_compile_features(nudge_shell PRIVATE cxx_std_17)
