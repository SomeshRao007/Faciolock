cmake_minimum_required(VERSION 3.16)

project(faciolock-kcm VERSION 0.1.0)

find_package(ECM 6.0.0 REQUIRED NO_MODULE)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

include(KDEInstallDirs)
include(KDECMakeSettings)
include(KDECompilerSettings NO_POLICY_SCOPE)

find_package(Qt6 6.5.0 REQUIRED COMPONENTS Core Quick)
find_package(KF6 6.0.0 REQUIRED COMPONENTS CoreAddons KCMUtils)

# Bakes ui/ into a Qt resource inside the plugin and generates the systemsettings .desktop
# entry from kcm_faciolock.json, so neither is a file this project installs by hand.
kcmutils_add_qml_kcm(kcm_faciolock SOURCES faciolockkcm.cpp)

# KF6::KCMUtilsQuick, namespaced. Upstream's own example says `KF6KCMUtilsQuick`, which is
# the in-tree target name and is not exported -- and CMake does not complain, it passes the
# unknown name to the linker as -lKF6KCMUtilsQuick and the build fails later, on a missing
# header, for a reason that has nothing to do with headers.
target_link_libraries(kcm_faciolock
    Qt6::Core
    KF6::CoreAddons
    KF6::KCMUtilsQuick
)

# The generated entry is NoDisplay=true, which is right for System Settings and wrong for an
# app launcher. This is the visible one.
install(FILES org.faciolock.panel.desktop DESTINATION ${KDE_INSTALL_APPDIR})
