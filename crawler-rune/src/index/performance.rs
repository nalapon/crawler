//! Keep native signposts at input/publication/paint, rather than a late window notification.
unsafe extern "C" {
    fn crawler_search_begin() -> u64;
    fn crawler_search_end(id: u64, cancelled: i32);
    fn crawler_search_published(id: u64, complete: i32, cached: i32);
}
#[derive(Default)]
pub struct Interval(Option<(u64, u64, std::time::Instant)>);
impl Interval {
    pub fn begin(&mut self, query: u64) -> u64 {
        self.cancel();
        // The shim uses public os_signpost macros and owns its process-lifetime log handle.
        let started = std::time::Instant::now();
        let id = unsafe { crawler_search_begin() };
        self.0 = Some((query, id, started));
        id
    }
    pub fn cancel(&mut self) {
        if let Some((_, id, _)) = self.0.take() {
            unsafe {
                crawler_search_end(id, 1);
            }
        }
    }
    pub fn elapsed(&self, query: u64) -> Option<f64> {
        self.0
            .filter(|(current, _, _)| *current == query)
            .map(|(_, _, at)| at.elapsed().as_secs_f64() * 1000.0)
    }
    pub fn drawn(&mut self, query: u64) {
        if self.0.is_some_and(|(current, _, _)| current == query)
            && let Some((_, id, _)) = self.0.take()
        {
            unsafe {
                crawler_search_end(id, 0);
            }
        }
    }
}
impl Drop for Interval {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub fn published(id: u64, complete: bool, cached: bool) {
    unsafe {
        crawler_search_published(id, i32::from(complete), i32::from(cached));
    }
}
