#import "VideoFallback.h"
#import <VLCKit/VLCKit.h>

// Serialize transport operations: stopping an old decoder never blocks AppKit,
// and the next selection cannot start decoding until the old player has stopped.
static dispatch_queue_t transportQueue(void) {
  static dispatch_queue_t queue;
  static dispatch_once_t once;
  dispatch_once(&once, ^{ queue = dispatch_queue_create("studio.crawler.video", DISPATCH_QUEUE_SERIAL); });
  return queue;
}

@interface CrawlerVLCPlayer () <VLCMediaPlayerDelegate>
@property(nonatomic) VLCMediaPlayer *player;
@property(nonatomic, readwrite) NSView *videoView;
@property(nonatomic) BOOL stopped;
@property(nonatomic) BOOL unplayable;
@property(nonatomic, readwrite) BOOL ready;
@end

@implementation CrawlerVLCPlayer
+ (BOOL)loadFrameworkAndReturnError:(NSError **)error {
  @synchronized(self) {
    if (!NSClassFromString(@"VLCMediaPlayer")) {
      NSString *path = [NSBundle.mainBundle.privateFrameworksPath stringByAppendingPathComponent:@"VLCKit.framework"];

      NSBundle *bundle = [NSBundle bundleWithPath:path];
      if (!bundle || ![bundle loadAndReturnError:error]) {
        if (error && !*error) *error = [NSError errorWithDomain:@"CrawlerVideo" code:1
          userInfo:@{NSLocalizedDescriptionKey: @"The video playback framework is missing. Rebuild Crawler with scripts/build.sh."}];
        return NO;
      }
    }
    // Warm the decoder library on the caller's background task, once per process.
    [NSClassFromString(@"VLCLibrary") sharedLibrary];
    return NSClassFromString(@"VLCMediaPlayer") != Nil;
  }
}

- (instancetype)initWithURL:(NSURL *)url {
  if ((self = [super init])) {
    _videoView = [[NSClassFromString(@"VLCVideoView") alloc] initWithFrame:NSZeroRect];
    _player = [[NSClassFromString(@"VLCMediaPlayer") alloc] init];
    _player.drawable = _videoView;
    _player.delegate = self;
    VLCMedia *media = [[NSClassFromString(@"VLCMedia") alloc] initWithURL:url];
    [media addOption:@":start-paused"];
    [media addOption:@":file-caching=300"];
    [media addOption:@":avcodec-hw=any"];
    _player.media = media;
  }
  return self;
}
- (BOOL)playing { return self.player.isPlaying; }
- (BOOL)failed { return self.unplayable || self.player.state == VLCMediaPlayerStateError; }
- (BOOL)seekable { return self.player.isSeekable; }
- (double)seconds { return MAX(0, self.player.time.intValue / 1000.0); }
- (double)duration { return MAX(0, self.player.media.length.intValue / 1000.0); }
- (float)position { return self.player.position; }
- (void)setPosition:(float)value {
  VLCMediaPlayer *player = self.player;
  dispatch_async(transportQueue(), ^{
    player.position = value;
    if (!player.isPlaying) [player gotoNextFrame];
  });
}
- (BOOL)muted { return self.player.audio.isMuted; }
- (void)setMuted:(BOOL)value { self.player.audio.muted = value; }
- (void)prepare { [self play]; }
- (void)play {
  if (self.stopped) return;
  VLCMediaPlayer *player = self.player;
  dispatch_async(transportQueue(), ^{ [player play]; });
}
- (void)pause {
  VLCMediaPlayer *player = self.player;
  dispatch_async(transportQueue(), ^{ [player pause]; });
}
- (void)stop {
  if (self.stopped) return;
  self.stopped = YES;
  self.changed = nil;
  self.player.delegate = nil;
  VLCMediaPlayer *player = self.player;
  // Keep the drawable alive until libVLC finishes releasing its output thread.
  NSView *view = self.videoView;
  dispatch_async(transportQueue(), ^{
    [player stop];
    dispatch_async(dispatch_get_main_queue(), ^{ player.drawable = nil; [view removeFromSuperview]; });
  });
}
- (void)mediaPlayerStateChanged:(NSNotification *)notification {
  if (self.stopped) return;
  if (!self.ready && self.player.state == VLCMediaPlayerStatePaused) {
    if (self.player.numberOfVideoTracks == 0) {
      self.unplayable = YES;
      if (self.changed) self.changed();
      return;
    }
    self.ready = YES;
    VLCMediaPlayer *player = self.player;
    dispatch_async(transportQueue(), ^{ [player gotoNextFrame]; });
  }
  if (self.changed) self.changed();
}
- (void)mediaPlayerTimeChanged:(NSNotification *)notification {
  if (!self.stopped && self.changed) self.changed();
}
@end
