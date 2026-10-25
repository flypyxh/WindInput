// 从 octagram `.gram` 里挑出「真实输入里用得到」的条目，重建成同格式的小模型。
//
//   g++ -O2 -std=c++17 -I<darts-clone/include> -o gram_select gram_select.cc
//   gram_select in.gram out.gram MIN_SCORE corpus.seg:WEIGHT [corpus.seg:WEIGHT ...]
//
// 每份语料（corpus_segment.py 的输出：一行一句、词间空格，标点与 | 断句）按正确切分模拟
// wind-engine `pinyin/octagram.rs` 的查询：context 取尾部 ≤3 字逐级缩短，word 取首部 ≤3 字做
// 前缀查找，取最高分那条（总长不足 2 字且非整串匹配的扣 12），句末另查 `word$`（扣 6）。
// 只给胜出的那条记分（每次 + 该语料的 WEIGHT），总分 ≥ MIN_SCORE 的条目留下。
//
// 为什么不按模型里的值裁：万象的值是分档分数（上千万条同档），分不出高低；且没出现在正确
// 路径上的搭配只会给错误路径加分。见 docs/design/language-model-integration.md §10.3。
//
// ⚠️ darts-clone 建的是 DAWG，不同的键共用叶单元，只能按完整键计分，不能按单元下标。
#include "darts.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <string>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unordered_map>
#include <vector>

namespace {

const uint32_t* A;
size_t N;
std::unordered_map<std::string, uint64_t> scores;

bool has_leaf(uint32_t u) { return (u >> 8) & 1; }
uint32_t value_of(uint32_t u) { return u & 0x7FFFFFFF; }
uint32_t offset_of(uint32_t u) { return (u >> 10) << ((u & 0x200) >> 6); }
uint32_t label_of(uint32_t u) { return u & 0x800000FF; }

long traverse(const std::string& key, size_t from, size_t to) {
  size_t idx = 0;
  uint32_t u = A[0];
  for (size_t i = from; i < to; i++) {
    unsigned char b = key[i];
    idx ^= offset_of(u) ^ b;
    if (idx >= N) return -1;
    u = A[idx];
    if (label_of(u) != b) return -1;
  }
  return static_cast<long>(idx);
}

struct Hit {
  uint32_t value;
  size_t bytes;
};

// 与 GramDb::common_prefix_search 相同：沿 key 走，收集途中每个成词节点（最多 8 个）
std::vector<Hit> prefix_hits(const std::string& key, size_t node) {
  std::vector<Hit> out;
  size_t idx = node;
  uint32_t u = A[idx];
  for (size_t i = 0; i < key.size() && out.size() < 8; i++) {
    unsigned char b = key[i];
    idx ^= offset_of(u) ^ b;
    if (idx >= N) break;
    u = A[idx];
    if (label_of(u) != b) break;
    if (has_leaf(u)) out.push_back({value_of(A[idx ^ offset_of(u)]), i + 1});
  }
  return out;
}

// gram_encoding.cc::encode
void encode(uint32_t u, std::string& out) {
  if (u < 0x80) {
    out.push_back(u == 0 ? static_cast<char>(0xE0) : static_cast<char>(u));
  } else if (u >= 0x4000 && u < 0xA000) {
    if ((u & 0xFF) == 0) {
      out.push_back(static_cast<char>(0xE1));
      out.push_back(static_cast<char>((u >> 8) + 0x40));
    } else {
      out.push_back(static_cast<char>((u >> 8) + 0x40));
      out.push_back(static_cast<char>(u & 0xFF));
    }
  } else {
    int bits = 32;
    uint32_t v = u;
    while (bits > 0 && (v & 0xFE000000) == 0) {
      bits -= 7;
      v <<= 7;
    }
    int n = (bits + 6) / 7;
    out.push_back(static_cast<char>(0xE0 | n));
    while (n-- > 0) {
      out.push_back(static_cast<char>(((v >> 25) & 0x7F) | 0x80));
      v <<= 7;
    }
  }
}

size_t encoded_char_len(unsigned char b) {
  if (b < 0x80 || b == 0xE0) return 1;
  if (b < 0xE0 || b == 0xE1) return 2;
  return 1 + (b & 0x0F);
}

std::vector<uint32_t> utf8(const std::string& s) {
  std::vector<uint32_t> r;
  for (size_t i = 0; i < s.size();) {
    unsigned char c = s[i];
    uint32_t u;
    int n;
    if (c < 0x80) { u = c; n = 1; }
    else if ((c >> 5) == 6) { u = c & 0x1F; n = 2; }
    else if ((c >> 4) == 14) { u = c & 0x0F; n = 3; }
    else { u = c & 0x07; n = 4; }
    for (int k = 1; k < n && i + k < s.size(); k++) u = (u << 6) | (s[i + k] & 0x3F);
    r.push_back(u);
    i += n;
  }
  return r;
}

bool all_han(const std::vector<uint32_t>& w) {
  for (uint32_t c : w)
    if (c < 0x4E00 || c > 0x9FFF) return false;
  return !w.empty();
}

const size_t kChars = 3;  // collocation_max_length(4) − 1

void score_word(const std::vector<uint32_t>& ctx, const std::vector<uint32_t>& word, bool rear,
                uint64_t weight) {
  if (ctx.empty()) return;
  std::string wk;
  size_t wl = std::min(kChars, word.size());
  for (size_t i = 0; i < wl; i++) encode(word[i], wk);
  std::string ck;
  std::vector<size_t> starts;
  for (size_t i = ctx.size() > kChars ? ctx.size() - kChars : 0; i < ctx.size(); i++) {
    starts.push_back(ck.size());
    encode(ctx[i], ck);
  }
  double best = 0;
  std::string best_key;
  for (size_t lv = 0; lv < starts.size(); lv++) {
    long node = traverse(ck, starts[lv], ck.size());
    if (node < 0) continue;
    size_t ctx_len = starts.size() - lv;
    for (const Hit& h : prefix_hits(wk, static_cast<size_t>(node))) {
      size_t match = 0;
      for (size_t b = 0; b < h.bytes; b += encoded_char_len(wk[b])) match++;
      bool whole = lv == 0 && h.bytes == wk.size();
      double s = h.value / 10000.0 + ((ctx_len + match >= 2 || whole) ? 0 : -12);
      if (s > best) {
        best = s;
        best_key = ck.substr(starts[lv]) + wk.substr(0, h.bytes);
      }
    }
  }
  if (rear && wl == word.size()) {
    long node = traverse(wk, 0, wk.size());
    if (node >= 0) {
      auto hs = prefix_hits("$", static_cast<size_t>(node));
      if (!hs.empty() && hs[0].value / 10000.0 - 6 > best) best_key = wk + "$";
    }
  }
  if (!best_key.empty()) scores[best_key] += weight;
}

void count_corpus(const char* path, uint64_t weight) {
  std::ifstream in(path);
  if (!in) {
    fprintf(stderr, "读不了 %s\n", path);
    exit(1);
  }
  std::string line;
  std::vector<std::vector<uint32_t>> clause;
  auto flush = [&]() {
    std::vector<uint32_t> ctx;
    for (size_t i = 0; i < clause.size(); i++) {
      score_word(ctx, clause[i], i + 1 == clause.size(), weight);
      ctx.insert(ctx.end(), clause[i].begin(), clause[i].end());
    }
    clause.clear();
  };
  size_t lines = 0;
  while (std::getline(in, line)) {
    lines++;
    size_t p = 0;
    while (p <= line.size()) {
      size_t q = line.find(' ', p);
      if (q == std::string::npos) q = line.size();
      if (q > p) {
        auto w = utf8(line.substr(p, q - p));
        if (all_han(w)) clause.push_back(std::move(w));
        else flush();
      }
      p = q + 1;
    }
    flush();
  }
  fprintf(stderr, "%s：%zu 行，权重 %llu\n", path, lines, static_cast<unsigned long long>(weight));
}

struct Entry {
  std::string key;
  uint32_t value;
};
std::vector<Entry> kept;
uint64_t min_score;

// 前序遍历：键按字节序升序产出，正是 darts-clone build 要求的顺序
void collect(size_t idx, std::string& key) {
  uint32_t u = A[idx];
  size_t base = idx ^ offset_of(u);
  if (has_leaf(u)) {
    auto it = scores.find(key);
    if (it != scores.end() && it->second >= min_score) kept.push_back({key, value_of(A[base])});
  }
  for (int b = 1; b < 256; b++) {
    size_t c = base ^ static_cast<size_t>(b);
    if (c >= N || label_of(A[c]) != static_cast<uint32_t>(b)) continue;
    key.push_back(static_cast<char>(b));
    collect(c, key);
    key.pop_back();
  }
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 5) {
    fprintf(stderr, "用法: %s in.gram out.gram MIN_SCORE corpus.seg:WEIGHT ...\n", argv[0]);
    return 2;
  }
  int fd = open(argv[1], O_RDONLY);
  struct stat st;
  if (fd < 0 || fstat(fd, &st) != 0) {
    fprintf(stderr, "打不开 %s\n", argv[1]);
    return 1;
  }
  const char* m = static_cast<const char*>(mmap(nullptr, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0));
  if (strncmp(m, "Rime::Grammar/", 14) != 0) {
    fprintf(stderr, "不是 rime 语法模型\n");
    return 1;
  }
  uint32_t da_size;
  int32_t off;
  memcpy(&da_size, m + 36, 4);
  memcpy(&off, m + 40, 4);
  A = reinterpret_cast<const uint32_t*>(m + 40 + off);
  N = da_size;

  min_score = strtoull(argv[3], nullptr, 10);
  for (int i = 4; i < argc; i++) {
    std::string arg = argv[i];
    size_t colon = arg.rfind(':');
    uint64_t w = colon == std::string::npos ? 1 : strtoull(arg.c_str() + colon + 1, nullptr, 10);
    count_corpus(arg.substr(0, colon).c_str(), w);
  }

  std::string key;
  collect(0, key);
  std::vector<const char*> keys;
  std::vector<size_t> lens;
  std::vector<int> values;
  for (const Entry& e : kept) {
    keys.push_back(e.key.c_str());
    lens.push_back(e.key.size());
    values.push_back(static_cast<int>(e.value));
  }
  Darts::DoubleArray da;
  if (da.build(keys.size(), keys.data(), lens.data(), values.data()) != 0) {
    fprintf(stderr, "双数组构建失败\n");
    return 1;
  }
  // gram_db.h::GramMetadata：format[32]、db_checksum、double_array_size、OffsetPtr（相对自身）
  FILE* f = fopen(argv[2], "wb");
  char format[32] = {0};
  strcpy(format, "Rime::Grammar/1.0");
  uint32_t checksum = 0, size = static_cast<uint32_t>(da.size());
  int32_t ptr = 4;
  fwrite(format, 1, sizeof format, f);
  fwrite(&checksum, 4, 1, f);
  fwrite(&size, 4, 1, f);
  fwrite(&ptr, 4, 1, f);
  fwrite(da.array(), da.unit_size(), da.size(), f);
  fclose(f);
  fprintf(stderr, "留下 %zu 条（被用到的共 %zu 条）→ %s，%.1f MB\n", kept.size(), scores.size(),
          argv[2], (44.0 + da.size() * da.unit_size()) / 1048576.0);
  return 0;
}
