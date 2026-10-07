// Fcitx5 waylandim 模块的公开函数声明（Fcitx5 没把 `waylandim_public.h` 装进开发包，这里照抄一份）。
//
// `ZwpInputMethodV2` 是 Fcitx5 私有的 C++ 包装类，开发包里也没有头文件：这里只前置声明，
// 当不透明指针传递，真正要的 C 层 `zwp_input_method_v2*` 由 WaylandPanel.cpp 的 `rawProxyOf` 取（带校验）。
#pragma once

#include <fcitx-utils/metastring.h>
#include <fcitx/addoninstance.h>
#include <fcitx/inputcontext.h>

namespace fcitx::wayland {
class ZwpInputMethodV2;
}

FCITX_ADDON_DECLARE_FUNCTION(WaylandIMModule, getInputMethodV2,
                             fcitx::wayland::ZwpInputMethodV2*(fcitx::InputContext*));
