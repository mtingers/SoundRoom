// Development-only native launcher for the portable TLS acceptance binary.
// It opens no audio hardware and changes no certificate trust settings.
#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>

static NSString *PrivatePassword(NSString *path) {
    int fd = open(path.fileSystemRepresentation, O_RDONLY | O_NOFOLLOW | O_NONBLOCK);
    if (fd < 0) return nil;
    struct stat info;
    if (fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) || info.st_uid != getuid() ||
        (info.st_mode & 077) != 0 || info.st_nlink != 1 || info.st_size < 1 || info.st_size > 4096) {
        close(fd); return nil;
    }
    NSMutableData *data = [NSMutableData dataWithLength:(NSUInteger)info.st_size];
    NSUInteger offset = 0;
    while (offset < data.length) {
        ssize_t count = read(fd, (char *)data.mutableBytes + offset, data.length - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { close(fd); return nil; }
        offset += (NSUInteger)count;
    }
    close(fd);
    NSString *password = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    return [password stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
}

@interface PortableDiagnostic : NSObject <NSApplicationDelegate>
@property NSTask *task;
@property NSFileHandle *log;
@property NSString *output;
@property NSString *selector;
@property NSTimeInterval began;
@property BOOL timedOut;
@property BOOL finished;
@property NSWindow *window;
@property NSTextField *status;
@end

@implementation PortableDiagnostic
- (void)finish:(int)status message:(NSString *)message {
    if (self.finished) return;
    self.finished = YES;
    [self.log closeFile];
    NSDictionary *result = @{
        @"exitCode": @(status), @"seconds": @(NSProcessInfo.processInfo.systemUptime - self.began),
        @"test": self.selector ?: @"", @"timedOut": @(self.timedOut),
        @"trustSettingsChanged": @NO, @"audioHardwareOpened": @NO,
        @"message": message ?: @""
    };
    NSData *json = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:nil];
    if (self.output) [json writeToFile:[self.output stringByAppendingPathExtension:@"json"] atomically:YES];
    self.status.stringValue = status == 0 ? @"Passed: portable acceptance. No hardware or trust changes." : [NSString stringWithFormat:@"Failed (%d): %@", status, message ?: @"See the saved QA log."];
}
- (void)quit:(id)sender { [NSApp terminate:nil]; }
- (void)applicationWillTerminate:(NSNotification *)notification { if (self.task.running) [self.task terminate]; }

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    self.began = NSProcessInfo.processInfo.systemUptime;
    self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 560, 110) styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable) backing:NSBackingStoreBuffered defer:NO];
    self.window.title = @"Soundroom portable acceptance";
    self.status = [NSTextField labelWithString:@"Running isolated acceptance. No audio hardware is opened."];
    self.status.frame = NSMakeRect(18, 52, 524, 40); self.status.maximumNumberOfLines = 2;
    [self.window.contentView addSubview:self.status];
    NSButton *quit = [NSButton buttonWithTitle:@"Quit" target:self action:@selector(quit:)]; quit.frame = NSMakeRect(455, 12, 85, 30); [self.window.contentView addSubview:quit];
    [self.window center]; [self.window makeKeyAndOrderFront:nil]; [NSApp activateIgnoringOtherApps:YES];
    NSDictionary *configuration = [NSBundle.mainBundle objectForInfoDictionaryKey:@"SoundroomPortableDiagnostic"];
    self.output = configuration[@"output"];
    self.selector = configuration[@"testSelector"];
    NSString *binary = configuration[@"testBinary"];
    NSString *tls = configuration[@"tlsDirectory"];
    // Only these reviewed tests are callable through the launcher. The fixture
    // password stays in its private source file and the child's environment.
    NSString *expected = @"service::request_acceptance::verified_wss_stop_while_large_authored_candidate_is_blocked";
    NSString *pairing = @"service::request_acceptance::verified_wss_pairing_continues_and_revoke_preserves_operator";
    if ((! [self.selector isEqualToString:expected] && ![self.selector isEqualToString:pairing]) || ![binary hasPrefix:@"/"] ||
        ![tls hasPrefix:@"/"] || ![self.output hasPrefix:@"/"]) {
        [self finish:2 message:@"Invalid diagnostic configuration"]; return;
    }
    NSString *password = PrivatePassword([tls stringByAppendingPathComponent:@"identity.password"]);
    if (password.length == 0) { [self finish:2 message:@"Private TLS password could not be read"]; return; }
    NSFileManager *files = NSFileManager.defaultManager;
    NSError *error = nil;
    if (![files createDirectoryAtPath:self.output.stringByDeletingLastPathComponent withIntermediateDirectories:YES attributes:nil error:&error] ||
        ![files createFileAtPath:self.output contents:nil attributes:@{NSFilePosixPermissions:@0600}]) {
        [self finish:2 message:@"Diagnostic log could not be created"]; return;
    }
    self.log = [NSFileHandle fileHandleForWritingAtPath:self.output];
    self.task = [NSTask new]; self.task.executableURL = [NSURL fileURLWithPath:binary];
    self.task.arguments = @[@"--exact", self.selector, @"--ignored", @"--nocapture"];
    NSMutableDictionary *environment = [NSProcessInfo.processInfo.environment mutableCopy];
    environment[@"SOUNDROOM_TEST_TLS_P12"] = [tls stringByAppendingPathComponent:@"server.p12"];
    environment[@"SOUNDROOM_TEST_TLS_PASSWORD"] = password;
    environment[@"SOUNDROOM_TEST_TLS_CA"] = [tls stringByAppendingPathComponent:@"ca.crt"];
    self.task.environment = environment;
    self.task.standardInput = NSFileHandle.fileHandleWithNullDevice;
    self.task.standardOutput = self.log; self.task.standardError = self.log;
    __weak PortableDiagnostic *weakSelf = self;
    self.task.terminationHandler = ^(NSTask *task) {
        int status = task.terminationStatus;
        dispatch_async(dispatch_get_main_queue(), ^{ [weakSelf finish:status message:nil]; });
    };
    if (![self.task launchAndReturnError:&error]) { [self finish:2 message:@"Diagnostic test could not launch"]; return; }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 180 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        PortableDiagnostic *owner = weakSelf;
        if (!owner || owner.finished || !owner.task.running) return;
        owner.timedOut = YES; [owner.task terminate];
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            if (!owner.finished && owner.task.running) kill(owner.task.processIdentifier, SIGKILL);
        });
    });
}
@end

int main(void) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
        PortableDiagnostic *delegate = [PortableDiagnostic new]; app.delegate = delegate;
        [app run];
    }
    return 0;
}
