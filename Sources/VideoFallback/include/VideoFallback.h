#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN
// The implementation loads VLCKit on demand. Crawler has no eager link to it.
@interface CrawlerVLCPlayer : NSObject
+ (BOOL)loadFrameworkAndReturnError:(NSError **)error;
- (instancetype)initWithURL:(NSURL *)url;
@property(nonatomic, readonly) NSView *videoView;
@property(nonatomic, copy, nullable) void (^changed)(void);
@property(nonatomic, readonly) BOOL playing;
@property(nonatomic, readonly) BOOL ready;
@property(nonatomic, readonly) BOOL failed;
@property(nonatomic, readonly) BOOL seekable;
@property(nonatomic, readonly) double seconds;
@property(nonatomic, readonly) double duration;
@property(nonatomic) float position;
@property(nonatomic) BOOL muted;
- (void)prepare;
- (void)play;
- (void)pause;
- (void)stop;
@end
NS_ASSUME_NONNULL_END
