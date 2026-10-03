# Bundled libaom 3.11.0, portable C build on MSVC; no assembler installation.
# These names are private to libaom; other workspace CMake users are unchanged.
set(AOM_TARGET_CPU "generic" CACHE STRING "ScreenMemory pinned libaom target" FORCE)
set(CONFIG_RUNTIME_CPU_DETECT 0 CACHE STRING "Portable libaom" FORCE)
set(CONFIG_TFLITE 0 CACHE STRING "No optional source fetch" FORCE)
set(FETCHCONTENT_FULLY_DISCONNECTED ON CACHE BOOL "Reviewed sources only" FORCE)
