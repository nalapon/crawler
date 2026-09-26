#import "SystemSettingsBridge.h"
#import <objc/message.h>
#import <dlfcn.h>

static NSError *Failure(NSString *message) {
  return [NSError errorWithDomain:@"Crawler.SystemSettings" code:1 userInfo:@{NSLocalizedDescriptionKey:message}];
}
// Validate the Objective-C signature before making a dynamically typed call.
static BOOL Signature(id object, SEL selector, const char *result, NSArray<NSString *> *arguments) {
  NSMethodSignature *signature = [object methodSignatureForSelector:selector];
  if (!signature || signature.numberOfArguments != arguments.count + 2 || strcmp(signature.methodReturnType, result)) return NO;
  for (NSUInteger i = 0; i < arguments.count; i++) {
    const char *type = [signature getArgumentTypeAtIndex:i + 2];
    while (*type == 'r' || *type == 'o' || *type == 'n' || *type == 'N') type++;
    if (strcmp(type, arguments[i].UTF8String)) return NO;
  }
  return YES;
}
static id Object(id object, NSString *name) {
  SEL selector = NSSelectorFromString(name);
  if (!Signature(object, selector, "@", @[])) return nil;
  return ((id (*)(id, SEL))objc_msgSend)(object, selector);
}
static BOOL ReadBoolean(id object, NSString *name) {
  SEL selector = NSSelectorFromString(name);
  if (!Signature(object, selector, @encode(BOOL), @[])) return NO;
  return ((BOOL (*)(id, SEL))objc_msgSend)(object, selector);
}
static NSArray *Endpoints(void) {
  // Loading Apple's framework does not patch it or bypass entitlement checks.
  static void *framework;
  static dispatch_once_t once;
  dispatch_once(&once, ^{ framework = dlopen("/System/Library/Frameworks/AVFoundation.framework/AVFoundation", RTLD_LAZY | RTLD_LOCAL); });
  if (!framework) return @[];
  Class provider = NSClassFromString(@"AVOutputContext");
  id context = Object(provider, @"sharedSystemAudioContext") ?: Object(provider, @"sharedSystemAudio");
  id devices = Object(context, @"outputDevices");
  if ([devices isKindOfClass:NSArray.class]) return devices;
  id device = Object(context, @"outputDevice");
  return device ? @[device] : @[];
}
static id Find(NSString *identifier) {
  for (id device in Endpoints()) if ([Object(device, @"deviceID") isEqual:identifier]) return device;
  return nil;
}
static void *BluetoothSymbol(const char *name) {
  static void *framework;
  static dispatch_once_t once;
  dispatch_once(&once, ^{ framework = dlopen("/System/Library/Frameworks/IOBluetooth.framework/IOBluetooth", RTLD_LAZY | RTLD_LOCAL); });
  return framework ? dlsym(framework, name) : NULL;
}
@implementation CRSystemSettingsBridge
+ (BOOL)canSetBluetoothPower { return BluetoothSymbol("IOBluetoothPreferenceSetControllerPowerState") != NULL; }
+ (BOOL)setBluetoothPower:(BOOL)enabled error:(NSError **)error {
  void (*setter)(int) = BluetoothSymbol("IOBluetoothPreferenceSetControllerPowerState");
  if (!setter) { if (error) *error = Failure(@"This macOS version has no compatible Bluetooth power control."); return NO; }
  setter(enabled ? 1 : 0);
  return YES;
}
+ (NSArray *)audioEndpointsWithError:(NSError **)error {
  @try {
    NSMutableArray *result = [NSMutableArray array];
    for (id device in Endpoints()) {
      id identifier = Object(device, @"deviceID");
      id modes = Object(device, @"availableBluetoothListeningModes");
      if (![identifier isKindOfClass:NSString.class] || ![modes isKindOfClass:NSArray.class] || [modes count] == 0) continue;
      NSMutableDictionary *row = [@{@"id":identifier, @"modes":modes} mutableCopy];
      id name = Object(device, @"name");
      id current = Object(device, @"currentBluetoothListeningMode");
      if ([name isKindOfClass:NSString.class]) row[@"name"] = name;
      if ([current isKindOfClass:NSString.class]) row[@"current"] = current;
      row[@"canSetMode"] = @(Signature(device, NSSelectorFromString(@"setCurrentBluetoothListeningMode:error:"), @encode(BOOL), @[@"@", @"^@"]));
      if (ReadBoolean(device, @"supportsConversationDetection") && Signature(device, NSSelectorFromString(@"setConversationDetectionEnabled:error:"), @encode(BOOL), @[@(@encode(BOOL)), @"^@"])) {
        row[@"conversation"] = @(ReadBoolean(device, @"isConversationDetectionEnabled"));
      }
      if (ReadBoolean(device, @"supportsHeadTrackedSpatialAudio") && Signature(device, NSSelectorFromString(@"setAllowsHeadTrackedSpatialAudio:error:"), @encode(BOOL), @[@(@encode(BOOL)), @"^@"])) {
        row[@"headTracking"] = @(ReadBoolean(device, @"allowsHeadTrackedSpatialAudio"));
      }
      [result addObject:row];
    }
    return result;
  } @catch (NSException *exception) { if (error) *error = Failure(@"macOS rejected discovery of advanced headphone controls."); return nil; }
}
+ (BOOL)setListeningMode:(NSString *)mode device:(NSString *)identifier error:(NSError **)error {
  @try {
    id device = Find(identifier);
    SEL selector = NSSelectorFromString(@"setCurrentBluetoothListeningMode:error:");
    id modes = Object(device, @"availableBluetoothListeningModes");
    if (![modes isKindOfClass:NSArray.class] || ![modes containsObject:mode] || !Signature(device, selector, @encode(BOOL), @[@"@", @"^@"])) {
      if (error) *error = Failure(@"This listening mode is not available for the connected device."); return NO;
    }
    return ((BOOL (*)(id, SEL, id, NSError **))objc_msgSend)(device, selector, mode, error);
  } @catch (NSException *exception) { if (error) *error = Failure(@"macOS rejected the listening mode request."); return NO; }
}
+ (BOOL)setConversationDetection:(BOOL)enabled device:(NSString *)identifier error:(NSError **)error {
  @try {
    id device = Find(identifier);
    SEL selector = NSSelectorFromString(@"setConversationDetectionEnabled:error:");
    if (!Signature(device, selector, @encode(BOOL), @[@(@encode(BOOL)), @"^@"])) {
      if (error) *error = Failure(@"Conversation awareness is unavailable for this device."); return NO;
    }
    return ((BOOL (*)(id, SEL, BOOL, NSError **))objc_msgSend)(device, selector, enabled, error);
  } @catch (NSException *exception) { if (error) *error = Failure(@"macOS rejected the conversation awareness request."); return NO; }
}
 + (BOOL)setHeadTracking:(BOOL)enabled device:(NSString *)identifier error:(NSError **)error {
  @try {
    id device = Find(identifier);
    SEL selector = NSSelectorFromString(@"setAllowsHeadTrackedSpatialAudio:error:");
    if (!Signature(device, selector, @encode(BOOL), @[@(@encode(BOOL)), @"^@"])) {
      if (error) *error = Failure(@"Head tracking is unavailable for this device."); return NO;
    }
    return ((BOOL (*)(id, SEL, BOOL, NSError **))objc_msgSend)(device, selector, enabled, error);
  } @catch (NSException *exception) { if (error) *error = Failure(@"macOS rejected the head tracking request."); return NO; }
}
@end
