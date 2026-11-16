#include <os/log.h>
#include <os/signpost.h>
#include <pthread.h>
#include <stdint.h>

static os_log_t search_log;
static pthread_once_t search_log_once = PTHREAD_ONCE_INIT;
static void initialize_search_log(void) {
    search_log = os_log_create("studio.crawler.rune", OS_LOG_CATEGORY_POINTS_OF_INTEREST);
}
static os_log_t log_handle(void) {
    pthread_once(&search_log_once, initialize_search_log);
    return search_log;
}
uint64_t crawler_search_begin(void) {
    os_log_t log = log_handle();
    uint64_t id = os_signpost_id_generate(log);
    os_signpost_interval_begin(log, id, "Search to first title draw");
    return id;
}
void crawler_search_end(uint64_t id, int cancelled) {
    os_signpost_interval_end(log_handle(), id, "Search to first title draw", "cancelled=%{public}d", cancelled);
}
void crawler_search_published(uint64_t id, int complete, int cached) {
    os_signpost_event_emit(log_handle(), id, "Results published", "complete=%{public}d cache=%{public}d", complete, cached);
}
