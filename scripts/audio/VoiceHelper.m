#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
// Private bounded JSON-lines helper. Never logs credentials or network diagnostics.
static dispatch_queue_t q,out;
static dispatch_semaphore_t slots,captureSlots;
static uint64_t sid,gen,epoch,playbackEpoch;
static BOOL muted=YES;
static AVAudioEngine *engine;
static AVAudioPlayerNode *player;
static NSURLSessionWebSocketTask *voiceSocket;
static NSUInteger playbackPending,networkPending;
static void emit(NSDictionary *event){NSData*d=[NSJSONSerialization dataWithJSONObject:event options:0 error:NULL];if(!d||d.length>131072||dispatch_semaphore_wait(slots,DISPATCH_TIME_NOW))return;dispatch_async(out,^{@try{[[NSFileHandle fileHandleWithStandardOutput]writeData:d];[[NSFileHandle fileHandleWithStandardOutput]writeData:[NSData dataWithBytes:"\n" length:1]];}@catch(NSException*e){}dispatch_semaphore_signal(slots);});}
static void fail(uint64_t id,NSString*message){emit(@{@"event":@"error",@"session":@(id),@"message":message});}
static void stop(void){epoch++;playbackEpoch++;muted=YES;if(engine){[engine.inputNode removeTapOnBus:0];[engine stop];}[player stop];engine=nil;player=nil;[voiceSocket cancelWithCloseCode:NSURLSessionWebSocketCloseCodeNormalClosure reason:nil];voiceSocket=nil;playbackPending=networkPending=0;}
static NSString* hex(NSData*d){const unsigned char*b=d.bytes;NSMutableString*s=[NSMutableString stringWithCapacity:d.length*2];for(NSUInteger i=0;i<d.length;i++)[s appendFormat:@"%02x",b[i]];return s;}
static NSData* unhex(NSString*s){if(![s isKindOfClass:NSString.class]||s.length>65536||s.length%4)return nil;NSMutableData*d=[NSMutableData dataWithCapacity:s.length/2];for(NSUInteger i=0;i<s.length;i+=2){unsigned n;NSScanner*scanner=[NSScanner scannerWithString:[s substringWithRange:NSMakeRange(i,2)]];if(![scanner scanHexInt:&n]||!scanner.isAtEnd)return nil;unsigned char b=n;[d appendBytes:&b length:1];}return d;}
static void receive(NSURLSessionWebSocketTask*ws,uint64_t id){[ws receiveMessageWithCompletionHandler:^(NSURLSessionWebSocketMessage*m,NSError*e){dispatch_async(q,^{if(sid!=id||voiceSocket!=ws)return;if(e){fail(id,@"voice connection closed");stop();return;}NSString*text=m.string?:[[NSString alloc]initWithData:m.data encoding:NSUTF8StringEncoding];if([text lengthOfBytesUsingEncoding:NSUTF8StringEncoding]>65536){fail(id,@"voice response too large");stop();return;}if(text)emit(@{@"event":@"message",@"session":@(id),@"json":text});receive(ws,id);});}];}
static void start(uint64_t id,NSString*endpoint,NSString*key){stop();sid=id;gen=0;muted=NO;uint64_t ticket=epoch;
 if(![endpoint isEqualToString:@"wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"]||!key.length){fail(id,@"invalid voice endpoint");return;}
 [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted){dispatch_async(q,^{if(ticket!=epoch||sid!=id)return;if(!granted){fail(id,@"microphone permission denied");return;}
 @try{AVAudioEngine*a=[AVAudioEngine new];AVAudioPlayerNode*p=[AVAudioPlayerNode new];[a attachNode:p];
 AVAudioFormat*play=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatFloat32 sampleRate:24000 channels:1 interleaved:NO];
 AVAudioFormat*cap=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:16000 channels:1 interleaved:YES];
 AVAudioConverter*converter=[[AVAudioConverter alloc]initFromFormat:[a.inputNode outputFormatForBus:0] toFormat:cap];if(!converter){fail(id,@"audio format unavailable");return;}
 [a connect:p to:a.mainMixerNode format:play];
 [a.inputNode installTapOnBus:0 bufferSize:4096 format:nil block:^(AVAudioPCMBuffer*b,AVAudioTime*t){
 AVAudioPCMBuffer*converted=[[AVAudioPCMBuffer alloc]initWithPCMFormat:cap frameCapacity:4096];__block BOOL used=NO;NSError*error=nil;
 [converter convertToBuffer:converted error:&error withInputFromBlock:^AVAudioBuffer*(AVAudioPacketCount n,AVAudioConverterInputStatus*status){if(used){*status=AVAudioConverterInputStatus_NoDataNow;return nil;}used=YES;*status=AVAudioConverterInputStatus_HaveData;return b;}];
 if(error||!converted.int16ChannelData||dispatch_semaphore_wait(captureSlots,DISPATCH_TIME_NOW))return;NSData*data=[NSData dataWithBytes:converted.int16ChannelData[0] length:converted.frameLength*2];
 dispatch_async(q,^{if(ticket==epoch&&sid==id&&!muted){for(NSUInteger off=0;off<data.length;off+=8192){NSData*chunk=[data subdataWithRange:NSMakeRange(off,MIN(8192,data.length-off))];emit(@{@"event":@"audio",@"session":@(id),@"pcm":hex(chunk)});}}dispatch_semaphore_signal(captureSlots);});
 }];
 NSError*error=nil;[a prepare];if(![a startAndReturnError:&error]){[a.inputNode removeTapOnBus:0];fail(id,@"audio device unavailable");return;}engine=a;player=p;[p play];
 NSURLComponents*parts=[NSURLComponents componentsWithString:endpoint];parts.queryItems=@[[NSURLQueryItem queryItemWithName:@"key" value:key]];
 voiceSocket=[[NSURLSession sessionWithConfiguration:NSURLSessionConfiguration.ephemeralSessionConfiguration]webSocketTaskWithURL:parts.URL];voiceSocket.maximumMessageSize=65536;[voiceSocket resume];receive(voiceSocket,id);emit(@{@"event":@"ready",@"session":@(id)});
 }@catch(NSException*e){stop();fail(id,@"audio device unavailable");}
 });}];}
static void handle(NSDictionary*c){uint64_t id=[c[@"session"]unsignedLongLongValue];NSString*op=c[@"op"];
 if([op isEqual:@"start"]){start(id,c[@"endpoint"],c[@"api_key"]);return;}if(id!=sid)return;
 if([op isEqual:@"stop"]){stop();sid=0;emit(@{@"event":@"stopped",@"session":@(id)});}
 else if([op isEqual:@"mute"]){muted=[c[@"muted"]boolValue];}
 else if([op isEqual:@"interrupt"]){gen=[c[@"generation"]unsignedLongLongValue];playbackEpoch++;[player stop];[player play];playbackPending=0;}
 else if([op isEqual:@"send"]){NSString*text=c[@"json"];if(!voiceSocket||networkPending>=32||[text lengthOfBytesUsingEncoding:NSUTF8StringEncoding]>65536){fail(id,@"voice send queue unavailable");stop();return;}networkPending++;NSURLSessionWebSocketTask*ws=voiceSocket;[ws sendMessage:[[NSURLSessionWebSocketMessage alloc]initWithString:text]completionHandler:^(NSError*e){dispatch_async(q,^{if(sid!=id||voiceSocket!=ws)return;if(networkPending)networkPending--;if(e){fail(id,@"voice send failed");stop();}});}];}
 else if([op isEqual:@"playback"]){uint64_t g=[c[@"generation"]unsignedLongLongValue];NSData*d=unhex(c[@"pcm"]);if(!d||g<gen||!player||playbackPending>=32)return;gen=g;uint64_t playbackTicket=playbackEpoch;AVAudioFormat*f=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatFloat32 sampleRate:24000 channels:1 interleaved:NO];AVAudioPCMBuffer*b=[[AVAudioPCMBuffer alloc]initWithPCMFormat:f frameCapacity:(AVAudioFrameCount)(d.length/2)];b.frameLength=(AVAudioFrameCount)(d.length/2);const int16_t*pcm=d.bytes;for(NSUInteger i=0;i<d.length/2;i++)b.floatChannelData[0][i]=(float)pcm[i]/32768.0f;playbackPending++;[player scheduleBuffer:b completionCallbackType:AVAudioPlayerNodeCompletionDataPlayedBack completionHandler:^(AVAudioPlayerNodeCompletionCallbackType t){dispatch_async(q,^{if(sid==id&&playbackEpoch==playbackTicket&&playbackPending)playbackPending--;});}];}}
int main(void){@autoreleasepool{q=dispatch_queue_create("alloy.voice",NULL);out=dispatch_queue_create("alloy.voice.output",NULL);slots=dispatch_semaphore_create(32);captureSlots=dispatch_semaphore_create(4);NSMutableData*line=[NSMutableData data];NSFileHandle*input=NSFileHandle.fileHandleWithStandardInput;while(YES){@autoreleasepool{NSData*chunk=[input readDataOfLength:4096];if(!chunk.length)break;const unsigned char*bytes=chunk.bytes;for(NSUInteger i=0;i<chunk.length;i++){if(bytes[i]==10){NSDictionary*c=[NSJSONSerialization JSONObjectWithData:line options:0 error:NULL];if([c isKindOfClass:NSDictionary.class])dispatch_sync(q,^{handle(c);});[line setLength:0];}else{[line appendBytes:bytes+i length:1];if(line.length>131072)return 2;}}}}dispatch_sync(q,^{stop();});dispatch_sync(out,^{});}return 0;}
