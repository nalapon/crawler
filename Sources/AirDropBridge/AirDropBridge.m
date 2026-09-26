#import "AirDropBridge.h"
#import <dns_sd.h>
#import <arpa/inet.h>
#import <net/if.h>
@interface CrawlerAirDropDevice ()
@property(nonatomic, copy) NSString *identifier, *name, *detail, *host;
@property(nonatomic) uint16_t port;
@property(nonatomic) uint32_t interfaceIndex;
@end
@implementation CrawlerAirDropDevice
@end
@interface AirDropResolution : NSObject
@property(nonatomic, weak) CrawlerAirDrop *owner;
@property(nonatomic, copy) NSString *identifier;
@property(nonatomic) DNSServiceRef service, addressService;
@property(nonatomic) uint16_t port;
@end
@implementation AirDropResolution
@end
@interface CrawlerAirDrop () {
    DNSServiceRef _browser;
}
@property(nonatomic, copy) NSArray<CrawlerAirDropDevice *> *devices;
@property(nonatomic, copy) NSString *failure;
@property(nonatomic, strong) NSMutableDictionary<NSString *, AirDropResolution *> *resolutions;
- (void)found:(const char *)name type:(const char *)type domain:(const char *)domain interface:(uint32_t)interface flags:(DNSServiceFlags)flags error:(DNSServiceErrorType)error;
- (void)address:(AirDropResolution *)resolution value:(const struct sockaddr *)address interface:(uint32_t)interface error:(DNSServiceErrorType)error;
- (void)resolved:(AirDropResolution *)resolution host:(const char *)host port:(uint16_t)port interface:(uint32_t)interface error:(DNSServiceErrorType)error;
@end
static void addressFound(DNSServiceRef service, DNSServiceFlags flags, uint32_t interface, DNSServiceErrorType error, const char *hostname, const struct sockaddr *address, uint32_t ttl, void *context) {
    AirDropResolution *resolution = (__bridge AirDropResolution *)context;
    if (flags & kDNSServiceFlagsAdd) [resolution.owner address:resolution value:address interface:interface error:error];
}
static void resolved(DNSServiceRef service, DNSServiceFlags flags, uint32_t interface, DNSServiceErrorType error, const char *fullname, const char *host, uint16_t port, uint16_t txtLength, const unsigned char *txt, void *context) {
    AirDropResolution *resolution = (__bridge AirDropResolution *)context;
    [resolution.owner resolved:resolution host:host port:port interface:interface error:error];
}
static void found(DNSServiceRef service, DNSServiceFlags flags, uint32_t interface, DNSServiceErrorType error, const char *name, const char *type, const char *domain, void *context) {
    [(__bridge CrawlerAirDrop *)context found:name type:type domain:domain interface:interface flags:flags error:error];
}
@implementation CrawlerAirDrop
- (instancetype)init {
    if ((self = [super init])) { _devices = @[]; _resolutions = [NSMutableDictionary dictionary]; }
    return self;
}
- (void)dealloc { [self stop]; }
- (void)start {
    if (_browser) return;
    self.failure = nil;
    // AutoTrigger asks Bonjour to start an AWDL browse when BLE discovers the service.
    DNSServiceErrorType error = DNSServiceBrowse(&_browser, kDNSServiceFlagsAutoTrigger | kDNSServiceFlagsIncludeAWDL | kDNSServiceFlagsIncludeP2P, kDNSServiceInterfaceIndexAny, "_airdrop._tcp", "local.", found, (__bridge void *)self);
    if (!error) error = DNSServiceSetDispatchQueue(_browser, dispatch_get_main_queue());
    if (error) {
        if (_browser) { DNSServiceRefDeallocate(_browser); _browser = NULL; }
        self.failure = [NSString stringWithFormat:@"AirDrop network discovery failed (%d).", error];
    }
    if (self.devicesChanged) self.devicesChanged();
}
- (void)stop {
    if (_browser) { DNSServiceRefDeallocate(_browser); _browser = NULL; }
    for (AirDropResolution *resolution in self.resolutions.allValues) { if (resolution.service) DNSServiceRefDeallocate(resolution.service); if (resolution.addressService) DNSServiceRefDeallocate(resolution.addressService); }
    [self.resolutions removeAllObjects];
    self.devices = @[];
}
- (void)found:(const char *)name type:(const char *)type domain:(const char *)domain interface:(uint32_t)interface flags:(DNSServiceFlags)flags error:(DNSServiceErrorType)error {
    if (error) {
        self.failure = [NSString stringWithFormat:@"AirDrop network discovery failed (%d).", error];
        if (self.devicesChanged) self.devicesChanged();
        return;
    }
    NSString *identifier = [NSString stringWithFormat:@"%s.%s%s%%%u", name, type, domain, interface];
    if (!(flags & kDNSServiceFlagsAdd)) {
        AirDropResolution *resolution = self.resolutions[identifier];
        { if (resolution.service) DNSServiceRefDeallocate(resolution.service); if (resolution.addressService) DNSServiceRefDeallocate(resolution.addressService); }
        [self.resolutions removeObjectForKey:identifier];
        self.devices = [self.devices filteredArrayUsingPredicate:[NSPredicate predicateWithBlock:^BOOL(CrawlerAirDropDevice *device, NSDictionary *bindings) { return ![device.identifier isEqual:identifier]; }]];
        if (self.devicesChanged) self.devicesChanged();
        return;
    }
    if (self.resolutions[identifier]) return;
    AirDropResolution *resolution = [AirDropResolution new];
    resolution.owner = self; resolution.identifier = identifier;
    DNSServiceRef service = NULL;
    error = DNSServiceResolve(&service, kDNSServiceFlagsIncludeAWDL | kDNSServiceFlagsIncludeP2P, interface, name, type, domain, resolved, (__bridge void *)resolution);
    if (!error) error = DNSServiceSetDispatchQueue(service, dispatch_get_main_queue());
    if (error) { if (service) DNSServiceRefDeallocate(service); return; }
    resolution.service = service;
    self.resolutions[identifier] = resolution;
}
- (void)resolved:(AirDropResolution *)resolution host:(const char *)host port:(uint16_t)port interface:(uint32_t)interface error:(DNSServiceErrorType)error {
    if (error || !host || !self.resolutions[resolution.identifier]) return;
    if (resolution.addressService) return;
    resolution.port = ntohs(port);
    DNSServiceRef addressService = NULL;
    error = DNSServiceGetAddrInfo(&addressService, kDNSServiceFlagsIncludeAWDL | kDNSServiceFlagsIncludeP2P, interface, kDNSServiceProtocol_IPv6, host, addressFound, (__bridge void *)resolution);
    if (!error) error = DNSServiceSetDispatchQueue(addressService, dispatch_get_main_queue());
    if (error) { if (addressService) DNSServiceRefDeallocate(addressService); return; }
    resolution.addressService = addressService;
}
- (void)address:(AirDropResolution *)resolution value:(const struct sockaddr *)address interface:(uint32_t)interface error:(DNSServiceErrorType)error {
    if (error || !address || address->sa_family != AF_INET6 || !self.resolutions[resolution.identifier]) return;
    for (CrawlerAirDropDevice *existing in self.devices) if ([existing.identifier isEqual:resolution.identifier]) return;
    char buffer[INET6_ADDRSTRLEN], interfaceName[IF_NAMESIZE];
    const struct sockaddr_in6 *ipv6 = (const struct sockaddr_in6 *)address;
    if (!inet_ntop(AF_INET6, &ipv6->sin6_addr, buffer, sizeof(buffer))) return;
    NSString *host = @(buffer);
    if (IN6_IS_ADDR_LINKLOCAL(&ipv6->sin6_addr)) {
        if (!if_indextoname(interface, interfaceName)) return;
        host = [host stringByAppendingFormat:@"%%%s", interfaceName];
    }
    CrawlerAirDropDevice *device = [CrawlerAirDropDevice new];
    device.identifier = resolution.identifier; device.name = @"Nearby AirDrop device"; device.detail = @"Identifying…";
    device.host = host; device.port = resolution.port; device.interfaceIndex = interface;
    self.devices = [self.devices arrayByAddingObject:device];
    if (self.devicesChanged) self.devicesChanged();
}
- (void)identifyDevice:(NSString *)identifier name:(NSString *)name {
    for (CrawlerAirDropDevice *device in self.devices) if ([device.identifier isEqual:identifier]) { device.name = name; device.detail = @"AirDrop"; }
    if (self.devicesChanged) self.devicesChanged();
}
- (void)identificationFailed:(NSString *)identifier reason:(NSString *)reason {
    for (CrawlerAirDropDevice *device in self.devices) if ([device.identifier isEqual:identifier]) device.detail = reason;
    if (self.devicesChanged) self.devicesChanged();
}
@end
