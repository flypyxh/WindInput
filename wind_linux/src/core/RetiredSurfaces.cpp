#include "RetiredSurfaces.h"

namespace windlinux {

void RetiredSurfaces::retire(Id id)
{
    entries_.push_back({id, false});
}

void RetiredSurfaces::synced(Id id)
{
    for (Entry& e : entries_) {
        if (e.id == id) {
            e.synced = true;
        }
    }
}

void RetiredSurfaces::pointerEnter(Id id)
{
    pointerOn_ = id; // 进了别处也就离开了原来那个（离开事件可能没来）
}

void RetiredSurfaces::pointerLeave(Id)
{
    // 协议保证 leave 只针对当前焦点 surface；对象已销毁时 libwayland 交来的是 0，同样算离开
    pointerOn_ = 0;
}

std::vector<RetiredSurfaces::Id> RetiredSurfaces::takeDestroyable()
{
    std::vector<Id> out;
    std::vector<Entry> keep;
    for (const Entry& e : entries_) {
        if (e.synced && e.id != pointerOn_) {
            out.push_back(e.id);
        } else {
            keep.push_back(e);
        }
    }
    entries_.swap(keep);
    return out;
}

std::vector<RetiredSurfaces::Id> RetiredSurfaces::takeAll()
{
    std::vector<Id> out;
    for (const Entry& e : entries_) {
        out.push_back(e.id);
    }
    entries_.clear();
    pointerOn_ = 0;
    return out;
}

} // namespace windlinux
