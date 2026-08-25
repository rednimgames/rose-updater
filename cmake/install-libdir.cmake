# wv-sys links `<OUT_DIR>/lib`, but GNUInstallDirs installs to `lib64` on
# Fedora/RHEL/SUSE. Pin it so every distro builds.
# No CMAKE_SYSTEM_NAME here: that would flip CMAKE_CROSSCOMPILING.
set(CMAKE_INSTALL_LIBDIR "lib" CACHE PATH "" FORCE)
