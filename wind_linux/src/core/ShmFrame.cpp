#include "ShmFrame.h"

#include "Protocol.h"

#include <cstring>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

namespace windlinux {

bool ShmFrameReader::open(const std::string& name, size_t size)
{
    close();
    int fd = ::shm_open(name.c_str(), O_RDONLY, 0);
    if (fd < 0) {
        return false;
    }
    void* p = ::mmap(nullptr, size, PROT_READ, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED) {
        ::close(fd);
        return false;
    }
    fd_ = fd;
    ptr_ = p;
    size_ = size;
    return true;
}

void ShmFrameReader::close()
{
    if (ptr_) {
        ::munmap(ptr_, size_);
        ptr_ = nullptr;
    }
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

bool ShmFrameReader::snapshot(SharedFrame& out) const
{
    if (!ptr_ || size_ < sizeof(SharedRenderHeader)) {
        return false;
    }
    SharedRenderHeader h;
    std::memcpy(&h, ptr_, sizeof(h)); // packed 结构，拷出来再读，免得未对齐访问
    if (h.magic != SHARED_RENDER_MAGIC) {
        return false;
    }
    out.sequence = h.sequence;
    out.flags = h.flags;
    out.screenX = h.x;
    out.screenY = h.y;
    out.width = h.width;
    out.height = h.height;
    out.stride = h.stride;
    out.bgra.clear();
    if (h.dataSize == 0) {
        return true;
    }
    // 头里的尺寸自相矛盾或越出映射范围：宁可丢这一帧，也别越界读。
    uint64_t need = uint64_t(h.stride) * h.height;
    if (h.stride < uint64_t(h.width) * 4 || need > h.dataSize
        || sizeof(SharedRenderHeader) + uint64_t(h.dataSize) > size_) {
        return false;
    }
    const auto* pix = static_cast<const uint8_t*>(ptr_) + sizeof(SharedRenderHeader);
    out.bgra.assign(pix, pix + need);
    return true;
}

Rect placePanel(int32_t x, int32_t y, int32_t w, int32_t h, const Rect& wa, bool absolute)
{
    constexpr int32_t kCaretHeight = 18; // 同 macOS：拿不到真实光标高时的估值
    Rect r{x, y, w, h};
    int32_t right = wa.x + wa.w;
    int32_t bottom = wa.y + wa.h;
    if (r.x + w > right) {
        r.x = right - w;
    }
    if (r.x < wa.x) {
        r.x = wa.x;
    }
    if (!absolute && r.y + h > bottom) {
        r.y = y - kCaretHeight - h; // 翻到光标上方
    }
    if (r.y + h > bottom) {
        r.y = bottom - h;
    }
    if (r.y < wa.y) {
        r.y = wa.y;
    }
    return r;
}

int32_t hitTest(const std::vector<CandidateHitRect>& rects, int32_t px, int32_t py)
{
    for (const auto& r : rects) {
        if (r.contains(px, py)) {
            return r.index;
        }
    }
    return kNoHit;
}

} // namespace windlinux
