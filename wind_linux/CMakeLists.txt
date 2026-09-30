# wind_linux：清风输入法的 Fcitx5 addon（C++17）。
#
#   eval "$(scripts/linux/bootstrap-sdk.sh env)"      # 无 root 机器上先引入用户前缀 SDK
#   cmake -S wind_linux -B wind_linux/build -G Ninja
#   cmake --build wind_linux/build
#   ctest --test-dir wind_linux/build --output-on-failure
#
# 选项：
#   WIND_BUILD_ADDON   构建 Fcitx5 addon（需 Fcitx5Core 开发包）；关掉则只编纯逻辑库与单测
#   WIND_VARIANT_DEV   编出 dev 变体（运行时目录 WindInputDev、SHM 名 /WindInput_SHMDev）
cmake_minimum_required(VERSION 3.16)
project(wind_linux VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

option(WIND_BUILD_ADDON "Build the Fcitx5 addon" ON)
option(WIND_VARIANT_DEV "Build the dev variant" OFF)

# ── 纯逻辑库（不依赖 Fcitx5，单测直接链接）──
add_library(windcore STATIC
    src/core/Bridge.cpp
    src/core/Codec.cpp
    src/core/HostUi.cpp
    src/core/KeyMap.cpp
    src/core/Menu.cpp
    src/core/OverlayInput.cpp
    src/core/ResponseRouter.cpp
    src/core/ServiceLauncher.cpp
    src/core/SettingsLauncher.cpp
    src/core/ShmFrame.cpp
    src/core/Utf.cpp
)
target_include_directories(windcore PUBLIC include)
target_compile_options(windcore PRIVATE -Wall -Wextra)
target_compile_definitions(windcore PRIVATE WIND_VARIANT_DEV=$<BOOL:${WIND_VARIANT_DEV}>)
find_package(Threads REQUIRED)
target_link_libraries(windcore PUBLIC Threads::Threads)

# ── 单测 ──
enable_testing()
foreach(t codec_test keymap_test router_test bridge_test protocol_sync_test shm_test launcher_test
        settings_launcher_test menu_test host_ui_test overlay_input_test)
    add_executable(${t} tests/${t}.cpp)
    target_link_libraries(${t} PRIVATE windcore)
    target_compile_definitions(${t} PRIVATE
        WIND_PROTOCOL_RS="${CMAKE_CURRENT_SOURCE_DIR}/../wind_input/crates/wind-ipc/src/protocol.rs")
    add_test(NAME ${t} COMMAND ${t})
endforeach()

# ── Fcitx5 addon ──
if(WIND_BUILD_ADDON)
    find_package(Fcitx5Core 5.0 REQUIRED)
    find_package(Fcitx5Utils 5.0 REQUIRED)
    find_package(PkgConfig REQUIRED)
    # 候选窗的 X11 呈现：自建 xcb 连接 + XShape（无合成器时按 alpha 抠形）。
    pkg_check_modules(XCB REQUIRED IMPORTED_TARGET xcb xcb-shape)

    add_library(windinput MODULE
        src/fcitx/WindEngine.cpp
        src/fcitx/X11Panel.cpp
    )
    target_link_libraries(windinput PRIVATE windcore Fcitx5::Core Fcitx5::Utils PkgConfig::XCB)
    target_compile_options(windinput PRIVATE -Wall -Wextra)
    # Fcitx5 按 addon conf 的 Library= 找 `lib<name>.so`；MODULE 默认就带 lib 前缀。
    set_target_properties(windinput PROPERTIES PREFIX "lib")

    # addon / 输入法描述文件：构建目录里也放一份，e2e 直接把构建目录当数据目录用，
    # 不必 install。
    configure_file(data/windinput-addon.conf.in
        ${CMAKE_CURRENT_BINARY_DIR}/share/fcitx5/addon/windinput.conf @ONLY)
    configure_file(data/windinput.conf.in
        ${CMAKE_CURRENT_BINARY_DIR}/share/fcitx5/inputmethod/windinput.conf @ONLY)

    include(GNUInstallDirs)
    install(TARGETS windinput DESTINATION ${CMAKE_INSTALL_LIBDIR}/fcitx5)
    install(FILES ${CMAKE_CURRENT_BINARY_DIR}/share/fcitx5/addon/windinput.conf
        DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/addon)
    install(FILES ${CMAKE_CURRENT_BINARY_DIR}/share/fcitx5/inputmethod/windinput.conf
        DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/inputmethod)
    # 图标（输入法条目 `windinput`、托盘随模式切换的 `windinput-zh*` / `-en` / `-caps`，以及
    # mimetypes/ 下方案包、主题包的文件类型图标）：成品入库，分别由 scripts/linux/gen-icons.py、
    # wind-ui 的 gen_tray_icons 例子与 scripts/linux/gen-mime-icons.py 一次性生成，构建与打包都
    # 不需要额外工具。
    install(DIRECTORY data/icons/hicolor DESTINATION ${CMAKE_INSTALL_DATADIR}/icons)
endif()
