#import <Foundation/Foundation.h>
NS_ASSUME_NONNULL_BEGIN
@interface CrawlerAirDropDevice : NSObject
@property(nonatomic, readonly, copy) NSString *identifier;
@property(nonatomic, readonly, copy) NSString *name;
@property(nonatomic, readonly, copy) NSString *detail;
@property(nonatomic, readonly, copy) NSString *host;
@property(nonatomic, readonly) uint16_t port;
@property(nonatomic, readonly) uint32_t interfaceIndex;
@end
// Public DNS-SD browsing explicitly includes Apple's peer-to-peer Wi-Fi interface.
// All callbacks and methods run on the main queue.
@interface CrawlerAirDrop : NSObject
@property(nonatomic, readonly, copy) NSArray<CrawlerAirDropDevice *> *devices;
@property(nonatomic, readonly, copy, nullable) NSString *failure;
@property(nonatomic, copy, nullable) void (^devicesChanged)(void);
- (void)start;
- (void)stop;
- (void)identifyDevice:(NSString *)identifier name:(NSString *)name;
- (void)identificationFailed:(NSString *)identifier reason:(NSString *)reason;
@end
NS_ASSUME_NONNULL_END
