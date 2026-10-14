// 退役 wl_surface 的销毁时机（纯逻辑，不依赖 libwayland）。
//
// input-method-v1 的面板角色（`zwp_input_panel_surface_v1`）没有 destroy 请求，要重新拿角色只能
// 换一个新 surface；KWin 又全局只认一个面板，Fcitx5 classicui 一显示就会无声顶掉我们的，所以
// v1 后端每个显示周期都换新 surface（见 docs/design/linux-kde-input-method-v1.md）。
//
// 旧 surface 不能立刻销毁：映射着的 surface 一销毁，迟到的 `wl_pointer.enter` 带的是已销毁对象
// （libwayland 交给回调的是 NULL），Fcitx5 classicui 的指针回调对它直接解引用就崩。所以先摘
// buffer 取消映射、发一次 `wl_display_sync`：sync 回来时合成器已处理完取消映射、此前发出的指针
// 事件也都已送达；再加上指针此刻不在它上面，才可以销毁。
#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>

namespace windlinux {

class RetiredSurfaces {
public:
    using Id = uintptr_t; // surface 指针值（只在 surface 存活期间唯一）；0 = 无

    /// 已摘 buffer 取消映射、已发 sync。
    void retire(Id id);
    /// 该 surface 退役时发的 sync 回来了。
    void synced(Id id);
    /// 指针进入某个 surface（不论是不是退役的）。
    void pointerEnter(Id id);
    /// 指针离开（参数只为对称；协议保证离开的就是当前焦点 surface）。
    void pointerLeave(Id id);
    /// 交出此刻可以销毁的（只交一次）。
    std::vector<Id> takeDestroyable();
    /// 连接拆除：不论状态全部交出。
    std::vector<Id> takeAll();
    size_t size() const { return entries_.size(); }

private:
    struct Entry {
        Id id;
        bool synced;
    };
    std::vector<Entry> entries_;
    Id pointerOn_ = 0;
};

} // namespace windlinux
