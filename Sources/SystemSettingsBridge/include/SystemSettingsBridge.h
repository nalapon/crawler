#import <Foundation/Foundation.h>
NS_ASSUME_NONNULL_BEGIN
@interface CRSystemSettingsBridge : NSObject
+ (BOOL)canSetBluetoothPower;
+ (BOOL)setBluetoothPower:(BOOL)enabled error:(NSError **)error;
+ (nullable NSArray<NSDictionary<NSString *, id> *> *)audioEndpointsWithError:(NSError **)error;
+ (BOOL)setListeningMode:(NSString *)mode device:(NSString *)identifier error:(NSError **)error;
+ (BOOL)setConversationDetection:(BOOL)enabled device:(NSString *)identifier error:(NSError **)error;
+ (BOOL)setHeadTracking:(BOOL)enabled device:(NSString *)identifier error:(NSError **)error;
@end
NS_ASSUME_NONNULL_END
