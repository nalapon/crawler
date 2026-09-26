#ifndef CRAWLER_FFF_BRIDGE_H
#define CRAWLER_FFF_BRIDGE_H
#include <stddef.h>
#include <stdint.h>

typedef struct CrawlerFFFIndex CrawlerFFFIndex;
typedef struct CrawlerFFFCache CrawlerFFFCache;
typedef struct { const uint8_t *data; size_t count; } CrawlerFFFBytes;
typedef struct {
  uint32_t index;
  int32_t score;
  uint32_t range_offset;
  uint32_t range_count;
} CrawlerFFFHit;
typedef struct { uint32_t start; uint32_t end; } CrawlerFFFRange;
typedef struct {
  const CrawlerFFFHit *hits;
  size_t hit_count;
  const CrawlerFFFRange *ranges;
  size_t range_count;
  const uint32_t *matching_indices;
  size_t total;
} CrawlerFFFResult;

// Frozen snapshots: searches may borrow an index concurrently. Release after the last search.
CrawlerFFFIndex *crawler_fff_create(const uint8_t *paths, size_t byte_count, size_t count);
void crawler_fff_destroy(CrawlerFFFIndex *index);
CrawlerFFFBytes *crawler_fff_serialize(const CrawlerFFFIndex *index);
CrawlerFFFBytes *crawler_fff_signature(const CrawlerFFFIndex *index);
// Cache view borrows this read-only mapping; do not pass it to free_bytes.
// Restored snapshots retain the mapping independently.
CrawlerFFFCache *crawler_fff_cache_open(const char *path);
CrawlerFFFBytes crawler_fff_cache_view(const CrawlerFFFCache *cache);
void crawler_fff_cache_destroy(CrawlerFFFCache *cache);
CrawlerFFFIndex *crawler_fff_restore_mapped(const CrawlerFFFCache *cache,
  size_t offset, size_t count, size_t expected_paths);
// 0 invalid, 1 valid ASCII, 2 valid non-ASCII UTF-8. No allocation or normalization.
uint8_t crawler_fff_validate_path(const uint8_t *path, size_t count);
void crawler_fff_free_bytes(CrawlerFFFBytes *bytes);
// Candidate IDs implement filesystem scope before FFF scoring and result truncation.
// NULL candidates uses all paths; NULL frecency uses unchanged zero-boost files.
CrawlerFFFResult *crawler_fff_search(const CrawlerFFFIndex *index, const char *query,
  const uint32_t *candidates, const int16_t *frecency, size_t count, size_t limit,
  uint8_t literal_words);
void crawler_fff_free_result(CrawlerFFFResult *result);
const char *crawler_fff_revision(void);
#endif
