// 退役 surface 的销毁时机：取消映射后要等合成器确认（一次 sync 往返）且指针不在上面，才能真正
// 销毁——否则迟到的指针事件会带着已销毁的 surface 落进 Fcitx5 classicui 的回调里。

#include "RetiredSurfaces.h"
#include "TestHarness.h"

#include <algorithm>

using namespace windlinux;

namespace {

bool contains(const std::vector<RetiredSurfaces::Id>& v, RetiredSurfaces::Id id)
{
    return std::find(v.begin(), v.end(), id) != v.end();
}

} // namespace

int main()
{
    CASE("刚退役、sync 还没回来：不销毁");
    {
        RetiredSurfaces r;
        r.retire(1);
        CHECK(r.takeDestroyable().empty());
        CHECK_EQ(r.size(), size_t(1));
    }

    CASE("sync 回来且指针不在上面：可销毁，且只交出一次");
    {
        RetiredSurfaces r;
        r.retire(1);
        r.synced(1);
        auto d = r.takeDestroyable();
        CHECK_EQ(d.size(), size_t(1));
        CHECK(contains(d, 1));
        CHECK(r.takeDestroyable().empty());
        CHECK_EQ(r.size(), size_t(0));
    }

    CASE("sync 回来但指针还在上面：等指针离开");
    {
        RetiredSurfaces r;
        r.pointerEnter(1);
        r.retire(1);
        r.synced(1);
        CHECK(r.takeDestroyable().empty());
        r.pointerLeave(1);
        CHECK(contains(r.takeDestroyable(), 1));
    }

    CASE("指针直接进了别的 surface（没收到离开）：也算离开");
    {
        RetiredSurfaces r;
        r.pointerEnter(1);
        r.retire(1);
        r.synced(1);
        r.pointerEnter(2);
        CHECK(contains(r.takeDestroyable(), 1));
    }

    CASE("离开事件带的是已销毁的 surface（libwayland 交来 0）：照样算离开");
    {
        // 协议保证 leave 只针对当前焦点 surface；不清零的话，旧地址被下一个新 surface 复用时
        // 它退役后会一直不回收
        RetiredSurfaces r;
        r.pointerEnter(1);
        r.retire(1);
        r.synced(1);
        r.pointerLeave(0);
        CHECK(contains(r.takeDestroyable(), 1));
    }

    CASE("多个退役 surface 各自独立");
    {
        RetiredSurfaces r;
        r.retire(1);
        r.retire(2);
        r.synced(2);
        auto d = r.takeDestroyable();
        CHECK_EQ(d.size(), size_t(1));
        CHECK(contains(d, 2));
        r.synced(1);
        CHECK(contains(r.takeDestroyable(), 1));
    }

    CASE("没退役过的 id 的 sync 不凭空生出条目");
    {
        RetiredSurfaces r;
        r.synced(7);
        CHECK(r.takeDestroyable().empty());
        CHECK_EQ(r.size(), size_t(0));
    }

    CASE("拆除：不论状态全部交出并清空");
    {
        RetiredSurfaces r;
        r.pointerEnter(1);
        r.retire(1);
        r.retire(2);
        r.synced(2);
        auto all = r.takeAll();
        CHECK_EQ(all.size(), size_t(2));
        CHECK(contains(all, 1) && contains(all, 2));
        CHECK_EQ(r.size(), size_t(0));
    }

    CASE("拆除后指针状态复位：同一地址再退役能正常回收");
    {
        RetiredSurfaces r;
        r.pointerEnter(1);
        r.retire(1);
        r.takeAll();
        r.retire(1);
        r.synced(1);
        CHECK(contains(r.takeDestroyable(), 1));
    }

    CASE("同一地址的 sync 标到所有同 id 条目上");
    {
        RetiredSurfaces r;
        r.retire(1);
        r.retire(2);
        r.synced(1);
        r.synced(2);
        CHECK_EQ(r.takeDestroyable().size(), size_t(2));
    }

    TEST_MAIN_END();
}
