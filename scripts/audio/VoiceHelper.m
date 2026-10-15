#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
#include <errno.h>
#include <unistd.h>
#include <math.h>
// Private bounded JSON-lines helper. Never logs credentials or network diagnostics.
static dispatch_queue_t q,out;
static dispatch_semaphore_t slots,captureSlots;
static uint64_t sid,gen,epoch,playbackEpoch;
static BOOL outputMuted=NO;
static BOOL testDirectAudio=NO,directAudio=NO,muted=YES,probeAudio=NO,probeNetwork=NO,providerReady=NO;
static const NSString *liveEndpoint=@"wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
static AVAudioEngine *engine;
static AVAudioPlayerNode *player;
static NSURLSessionWebSocketTask *voiceSocket;
static NSURLSession *socketSession;
static NSUInteger playbackPending,networkPending;
static dispatch_source_t statsTimer,testCaptureTimer;
static uint64_t capturedFrames,capturedSamples,emittedFrames,sentFrames,inputTranscripts,conversionErrors;
static uint16_t captureLevel;
static NSTimeInterval lastLevelTime;
static BOOL emit(NSDictionary *event){NSData*d=[NSJSONSerialization dataWithJSONObject:event options:0 error:NULL];if(!d||d.length>131072||dispatch_semaphore_wait(slots,DISPATCH_TIME_NOW))return NO;dispatch_async(out,^{@try{[[NSFileHandle fileHandleWithStandardOutput]writeData:d];[[NSFileHandle fileHandleWithStandardOutput]writeData:[NSData dataWithBytes:"\n" length:1]];}@catch(NSException*e){}dispatch_semaphore_signal(slots);});return YES;}
static void fail(uint64_t id,NSString*message){emit(@{@"event":@"error",@"session":@(id),@"message":message});if(probeAudio){dispatch_sync(out,^{});exit(1);}}
static void progress(uint64_t id,NSString*stage){emit(@{@"event":@"progress",@"session":@(id),@"stage":stage});}
static void stop(void){if(testCaptureTimer){dispatch_source_cancel(testCaptureTimer);testCaptureTimer=nil;}if(statsTimer){dispatch_source_cancel(statsTimer);statsTimer=nil;}providerReady=NO;epoch++;playbackEpoch++;muted=YES;if(engine){[engine.inputNode removeTapOnBus:0];[engine stop];}[player stop];engine=nil;player=nil;[voiceSocket cancelWithCloseCode:NSURLSessionWebSocketCloseCodeNormalClosure reason:nil];voiceSocket=nil;[socketSession invalidateAndCancel];socketSession=nil;playbackPending=networkPending=0;}
static void audioStats(void){emit(@{@"event":@"audio_stats",@"session":@(sid),@"captured_frames":@(capturedFrames),@"captured_samples":@(capturedSamples),@"emitted_frames":@(emittedFrames),@"sent_frames":@(sentFrames),@"input_transcripts":@(inputTranscripts),@"conversion_errors":@(conversionErrors),@"level":@(captureLevel),@"muted":@(muted),@"provider_ready":@(providerReady),@"direct_audio":@(directAudio)});}
static void startStats(uint64_t id,uint64_t ticket){
 statsTimer=dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER,0,0,q);
 dispatch_source_set_timer(statsTimer,dispatch_time(DISPATCH_TIME_NOW,NSEC_PER_SEC),NSEC_PER_SEC,100*NSEC_PER_MSEC);
 dispatch_source_set_event_handler(statsTimer,^{if(ticket==epoch&&sid==id)audioStats();});
 dispatch_resume(statsTimer);
}
static uint16_t pcmLevel(const int16_t*pcm,NSUInteger samples){
 if(!samples)return 0;double sum=0;for(NSUInteger i=0;i<samples;i++){double value=pcm[i]/32768.0;sum+=value*value;}
 return (uint16_t)MIN(1000.0,sqrt(sum/samples)*4000.0);
}
static NSString* hex(NSData*d){const unsigned char*b=d.bytes;NSMutableString*s=[NSMutableString stringWithCapacity:d.length*2];for(NSUInteger i=0;i<d.length;i++)[s appendFormat:@"%02x",b[i]];return s;}
static NSData* unhex(NSString*s){if(![s isKindOfClass:NSString.class]||s.length>65536||s.length%4)return nil;NSMutableData*d=[NSMutableData dataWithCapacity:s.length/2];for(NSUInteger i=0;i<s.length;i+=2){unsigned n;NSScanner*scanner=[NSScanner scannerWithString:[s substringWithRange:NSMakeRange(i,2)]];if(![scanner scanHexInt:&n]||!scanner.isAtEnd)return nil;unsigned char b=n;[d appendBytes:&b length:1];}return d;}
// Shared by real capture and the device-free conversion regression.
static AVAudioPCMBuffer* convertAudio(AVAudioConverter*converter,AVAudioPCMBuffer*input,AVAudioFormat*format,NSError**error){
 AVAudioPCMBuffer*converted=[[AVAudioPCMBuffer alloc]initWithPCMFormat:format frameCapacity:4096];__block BOOL used=NO;
 [converter convertToBuffer:converted error:error withInputFromBlock:^AVAudioBuffer*(AVAudioPacketCount n,AVAudioConverterInputStatus*status){if(used){*status=AVAudioConverterInputStatus_NoDataNow;return nil;}used=YES;*status=AVAudioConverterInputStatus_HaveData;return input;}];
 return converted;
}
static int testConverter(void){
 for(NSUInteger channels=1;channels<=2;channels++){
  double rate=channels==1?44100:48000;
  AVAudioFormat*inputFormat=[[AVAudioFormat alloc]initStandardFormatWithSampleRate:rate channels:(AVAudioChannelCount)channels];
  AVAudioFormat*outputFormat=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:16000 channels:1 interleaved:YES];
  AVAudioConverter*converter=[[AVAudioConverter alloc]initFromFormat:inputFormat toFormat:outputFormat];
  uint64_t samples=0;uint16_t maximumLevel=0;
  for(NSUInteger frame=0;frame<32;frame++){
   AVAudioPCMBuffer*input=[[AVAudioPCMBuffer alloc]initWithPCMFormat:inputFormat frameCapacity:4096];input.frameLength=4096;
   for(NSUInteger channel=0;channel<channels;channel++)for(NSUInteger i=0;i<4096;i++)input.floatChannelData[channel][i]=0.25*sin((frame*4096+i)*2*M_PI*440/rate);
   NSError*error=nil;AVAudioPCMBuffer*converted=convertAudio(converter,input,outputFormat,&error);
   if(error||!converted.int16ChannelData)return 1;
   samples+=converted.frameLength;maximumLevel=MAX(maximumLevel,pcmLevel(converted.int16ChannelData[0],converted.frameLength));
  }
  if(samples<40000||samples>50000||maximumLevel<100||maximumLevel>1000)return 1;
 }
 puts("converter: mono44.1k/stereo48k -> PCM16 mono16k passed");return 0;
}
@interface VoiceSocketDelegate:NSObject<NSURLSessionWebSocketDelegate>
@property(nonatomic) uint64_t session;
@end
@implementation VoiceSocketDelegate
- (void)URLSession:(NSURLSession*)session webSocketTask:(NSURLSessionWebSocketTask*)task didOpenWithProtocol:(NSString*)protocol {
 dispatch_async(q,^{if(sid==self.session&&voiceSocket==task)progress(sid,@"socket_open");});
}
@end
// Audio stays on the host; only control, captions and tool messages cross UART.
static void sendText(NSString*text,uint64_t id,BOOL isAudio,BOOL isSetup){
 if(!voiceSocket||networkPending>=32){fail(id,@"voice send queue unavailable");stop();return;}
 networkPending++;NSURLSessionWebSocketTask*ws=voiceSocket;
 [ws sendMessage:[[NSURLSessionWebSocketMessage alloc]initWithString:text]completionHandler:^(NSError*e){dispatch_async(q,^{if(sid!=id||voiceSocket!=ws)return;if(networkPending)networkPending--;if(e){fail(id,@"voice send failed");stop();}else{if(isAudio)sentFrames++;if(isSetup)progress(id,@"setup_sent");}});}];
}
static NSString* audioPayload(NSData*data){
 NSDictionary*payload=@{@"realtimeInput":@{@"audio":@{@"mimeType":@"audio/pcm;rate=16000",@"data":[data base64EncodedStringWithOptions:0]}}};
 return [[NSString alloc]initWithData:[NSJSONSerialization dataWithJSONObject:payload options:0 error:NULL] encoding:NSUTF8StringEncoding];
}
static void clearPlayback(void){playbackEpoch++;[player stop];[player play];playbackPending=0;if(directAudio)emit(@{@"event":@"playback_active",@"session":@(sid),@"active":@NO});}
static void playPCM(NSData*d,uint64_t id){
 if(outputMuted||!d||d.length%2)return;
 if(testDirectAudio){emit(@{@"event":@"playback_active",@"session":@(id),@"active":@YES});uint64_t ticket=epoch;dispatch_after(dispatch_time(DISPATCH_TIME_NOW,50*NSEC_PER_MSEC),q,^{if(ticket==epoch&&sid==id)emit(@{@"event":@"playback_active",@"session":@(id),@"active":@NO});});return;}
 if(!player)return;
 if(playbackPending>=32){fail(id,@"voice playback queue unavailable");stop();return;}
 uint64_t playbackTicket=playbackEpoch;
 AVAudioFormat*f=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatFloat32 sampleRate:24000 channels:1 interleaved:NO];
 AVAudioPCMBuffer*b=[[AVAudioPCMBuffer alloc]initWithPCMFormat:f frameCapacity:(AVAudioFrameCount)(d.length/2)];b.frameLength=(AVAudioFrameCount)(d.length/2);
 const int16_t*pcm=d.bytes;for(NSUInteger i=0;i<d.length/2;i++)b.floatChannelData[0][i]=(float)pcm[i]/32768.0f;
 if(!playbackPending&&directAudio)emit(@{@"event":@"playback_active",@"session":@(id),@"active":@YES});
 playbackPending++;
 [player scheduleBuffer:b completionCallbackType:AVAudioPlayerNodeCompletionDataPlayedBack completionHandler:^(AVAudioPlayerNodeCompletionCallbackType t){dispatch_async(q,^{if(sid==id&&playbackEpoch==playbackTicket&&playbackPending){playbackPending--;if(!playbackPending&&directAudio)emit(@{@"event":@"playback_active",@"session":@(id),@"active":@NO});}});}];
}
// Remove only successfully handled PCM fields; preserve mixed text/tool parts.
static NSDictionary* filterAudio(NSDictionary*message,void(^consume)(NSData*)){
 NSDictionary*content=message[@"serverContent"];if(![content isKindOfClass:NSDictionary.class])return message;
 NSDictionary*turn=content[@"modelTurn"];NSArray*parts=turn[@"parts"];if(![parts isKindOfClass:NSArray.class])return message;
 NSMutableArray*filtered=[NSMutableArray array];
 for(NSDictionary*part in parts){if(![part isKindOfClass:NSDictionary.class]){[filtered addObject:part];continue;}
 NSDictionary*inlineData=part[@"inlineData"];NSString*mime=inlineData[@"mimeType"];NSData*data=nil;
 if([mime isKindOfClass:NSString.class]&&[mime hasPrefix:@"audio/pcm"]&&[inlineData[@"data"] isKindOfClass:NSString.class])data=[[NSData alloc]initWithBase64EncodedString:inlineData[@"data"] options:0];
 if(data&&data.length&&data.length%2==0){consume(data);NSMutableDictionary*rest=[part mutableCopy];[rest removeObjectForKey:@"inlineData"];if(rest.count)[filtered addObject:rest];}else [filtered addObject:part];
 }
 NSMutableDictionary*newTurn=[turn mutableCopy];newTurn[@"parts"]=filtered;
 NSMutableDictionary*newContent=[content mutableCopy];newContent[@"modelTurn"]=newTurn;
 NSMutableDictionary*result=[message mutableCopy];result[@"serverContent"]=newContent;return result;
}
static void receive(NSURLSessionWebSocketTask*ws,uint64_t id){[ws receiveMessageWithCompletionHandler:^(NSURLSessionWebSocketMessage*m,NSError*e){dispatch_async(q,^{
 if(sid!=id||voiceSocket!=ws)return;
 if(e){fail(id,@"voice connection closed");stop();return;}
 NSString*text=m.string?:[[NSString alloc]initWithData:m.data encoding:NSUTF8StringEncoding];
 if([text lengthOfBytesUsingEncoding:NSUTF8StringEncoding]>1048576){fail(id,@"voice response too large");stop();return;}
 if(text){NSDictionary*message=[NSJSONSerialization JSONObjectWithData:[text dataUsingEncoding:NSUTF8StringEncoding] options:0 error:NULL];
 if([message isKindOfClass:NSDictionary.class]){
 if(message[@"setupComplete"])providerReady=YES;
 NSDictionary*content=message[@"serverContent"];
 if([content isKindOfClass:NSDictionary.class]){if(content[@"inputTranscription"])inputTranscripts++;if(directAudio&&[content[@"interrupted"]boolValue])clearPlayback();}
 if(directAudio){message=filterAudio(message,^(NSData*data){playPCM(data,id);});text=[[NSString alloc]initWithData:[NSJSONSerialization dataWithJSONObject:message options:0 error:NULL] encoding:NSUTF8StringEncoding];}
 }
 if([text lengthOfBytesUsingEncoding:NSUTF8StringEncoding]>65536){fail(id,@"voice control response too large");stop();return;}
 emit(@{@"event":@"message",@"session":@(id),@"json":text});}
 receive(ws,id);
 });}];}
static void connectVoice(uint64_t id,NSString*endpoint,NSString*key){
 NSURLComponents*parts=[NSURLComponents componentsWithString:endpoint];parts.queryItems=@[[NSURLQueryItem queryItemWithName:@"key" value:key]];
 VoiceSocketDelegate*delegate=[VoiceSocketDelegate new];delegate.session=id;socketSession=[NSURLSession sessionWithConfiguration:NSURLSessionConfiguration.ephemeralSessionConfiguration delegate:delegate delegateQueue:nil];voiceSocket=[socketSession webSocketTaskWithURL:parts.URL];voiceSocket.maximumMessageSize=1048576;[voiceSocket resume];receive(voiceSocket,id);emit(@{@"event":@"ready",@"session":@(id)});
}
// All sources, including fixtures, share production gating and transport.
static void captureData(NSData*data,BOOL conversionFailed,uint64_t id,uint64_t ticket){
 if(ticket!=epoch||sid!=id)return;
 NSUInteger samples=data.length/2;uint16_t level=conversionFailed?0:pcmLevel(data.bytes,samples);
 capturedFrames++;capturedSamples+=samples;captureLevel=level;if(conversionFailed)conversionErrors++;
 NSTimeInterval now=NSProcessInfo.processInfo.systemUptime;
 if(directAudio&&providerReady&&now-lastLevelTime>=0.1){lastLevelTime=now;emit(@{@"event":@"audio_level",@"session":@(id),@"level":@(muted?0:level)});}
 if(muted||probeAudio||!providerReady)return;
 for(NSUInteger off=0;off<data.length;off+=8192){
 NSData*chunk=[data subdataWithRange:NSMakeRange(off,MIN(8192,data.length-off))];
 if(directAudio){sendText(audioPayload(chunk),id,YES,NO);emittedFrames++;}
 else if(emit(@{@"event":@"audio",@"session":@(id),@"pcm":hex(chunk)}))emittedFrames++;
 }
}
static void start(uint64_t id,NSString*endpoint,NSString*key){stop();sid=id;gen=0;muted=NO;capturedFrames=capturedSamples=emittedFrames=sentFrames=inputTranscripts=conversionErrors=0;captureLevel=0;uint64_t ticket=epoch;
 if((![endpoint isEqualToString:(NSString*)liveEndpoint]&&!(testDirectAudio&&[endpoint hasPrefix:@"ws://127.0.0.1:"]))||!key.length){fail(id,@"invalid voice endpoint");return;}
 if(!directAudio&&!probeAudio&&!probeNetwork){fail(id,@"Voice transport mismatch; rebuild kernel and restart QEMU");return;}
 if(testDirectAudio){
 startStats(id,ticket);
 testCaptureTimer=dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER,0,0,q);
 dispatch_source_set_timer(testCaptureTimer,dispatch_time(DISPATCH_TIME_NOW,100*NSEC_PER_MSEC),100*NSEC_PER_MSEC,0);
 dispatch_source_set_event_handler(testCaptureTimer,^{if(ticket!=epoch||sid!=id)return;int16_t pcm[1600];for(NSUInteger i=0;i<1600;i++)pcm[i]=(int16_t)(8192*sin((capturedSamples+i)*2*M_PI*440/16000));captureData([NSData dataWithBytes:pcm length:sizeof(pcm)],NO,id,ticket);});
 dispatch_resume(testCaptureTimer);connectVoice(id,endpoint,key);return;
 }
 if(probeNetwork){connectVoice(id,endpoint,key);return;}
 progress(id,@"permission_pending");
 [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted){dispatch_async(q,^{if(ticket!=epoch||sid!=id)return;if(!granted){fail(id,@"microphone permission denied");return;}
 progress(id,@"permission_granted");progress(id,@"audio_starting");
 @try{AVAudioEngine*a=[AVAudioEngine new];AVAudioPlayerNode*p=[AVAudioPlayerNode new];[a attachNode:p];
 AVAudioFormat*play=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatFloat32 sampleRate:24000 channels:1 interleaved:NO];
 AVAudioFormat*cap=[[AVAudioFormat alloc]initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:16000 channels:1 interleaved:YES];
 AVAudioConverter*converter=[[AVAudioConverter alloc]initFromFormat:[a.inputNode outputFormatForBus:0] toFormat:cap];if(!converter){fail(id,@"audio format unavailable");return;}
 [a connect:p to:a.mainMixerNode format:play];
 [a.inputNode installTapOnBus:0 bufferSize:4096 format:nil block:^(AVAudioPCMBuffer*b,AVAudioTime*t){
 NSError*error=nil;AVAudioPCMBuffer*converted=convertAudio(converter,b,cap,&error);
 if(dispatch_semaphore_wait(captureSlots,DISPATCH_TIME_NOW))return;
 BOOL conversionFailed=error!=nil||!converted.int16ChannelData;
 NSUInteger samples=conversionFailed?0:converted.frameLength;
 NSData*data=conversionFailed?[NSData data]:[NSData dataWithBytes:converted.int16ChannelData[0] length:samples*2];
 dispatch_async(q,^{captureData(data,conversionFailed,id,ticket);dispatch_semaphore_signal(captureSlots);});
 }];
 NSError*error=nil;[a prepare];if(![a startAndReturnError:&error]){[a.inputNode removeTapOnBus:0];fail(id,[NSString stringWithFormat:@"audio device unavailable (code %ld)",(long)error.code]);return;}engine=a;player=p;[p play];progress(id,@"audio_ready");startStats(id,ticket);
 if(probeAudio){dispatch_after(dispatch_time(DISPATCH_TIME_NOW,3*NSEC_PER_SEC),q,^{if(ticket==epoch&&sid==id){audioStats();stop();emit(@{@"event":@"stopped",@"session":@(id)});dispatch_sync(out,^{});exit(0);}});return;}
 connectVoice(id,endpoint,key);
 }@catch(NSException*e){stop();fail(id,@"audio device unavailable");}
 });}];}
static void handle(NSDictionary*c){uint64_t id=[c[@"session"]unsignedLongLongValue];NSString*op=c[@"op"];
 if([op isEqual:@"start"]){outputMuted=[c[@"output_muted"]boolValue];directAudio=[c[@"direct_audio"]boolValue];start(id,c[@"endpoint"],c[@"api_key"]);return;}if(id!=sid)return;
 if([op isEqual:@"stop"]){stop();sid=0;emit(@{@"event":@"stopped",@"session":@(id)});}
 else if([op isEqual:@"output_mute"]){outputMuted=[c[@"muted"]boolValue];if(outputMuted)clearPlayback();}
 else if([op isEqual:@"mute"]){muted=[c[@"muted"]boolValue];}
 else if([op isEqual:@"interrupt"]){gen=[c[@"generation"]unsignedLongLongValue];clearPlayback();}
 else if([op isEqual:@"send"]){NSString*text=c[@"json"];if(![text isKindOfClass:NSString.class]||[text lengthOfBytesUsingEncoding:NSUTF8StringEncoding]>65536){fail(id,@"voice send too large");stop();return;}NSDictionary*payload=[NSJSONSerialization JSONObjectWithData:[text dataUsingEncoding:NSUTF8StringEncoding] options:0 error:NULL];BOOL setup=[payload isKindOfClass:NSDictionary.class]&&payload[@"setup"]!=nil;BOOL audio=[payload[@"realtimeInput"] isKindOfClass:NSDictionary.class]&&(payload[@"realtimeInput"][@"audio"]||payload[@"realtimeInput"][@"mediaChunks"]);sendText(text,id,audio,setup);}
 else if([op isEqual:@"playback"]){uint64_t g=[c[@"generation"]unsignedLongLongValue];if(g<gen)return;gen=g;playPCM(unhex(c[@"pcm"]),id);}}

int main(int argc,const char*argv[]) {
 @autoreleasepool {
  if(argc==2&&strcmp(argv[1],"--test-converter")==0)return testConverter();
  q=dispatch_queue_create("alloy.voice",NULL);
  out=dispatch_queue_create("alloy.voice.output",NULL);
  slots=dispatch_semaphore_create(32);
  captureSlots=dispatch_semaphore_create(4);
  testDirectAudio=argc==2&&strcmp(argv[1],"--test-direct-audio")==0;
  probeNetwork=argc==2&&strcmp(argv[1],"--probe-network")==0;
  if(argc==2&&strcmp(argv[1],"--probe-audio")==0){
   probeAudio=YES;
   dispatch_async(q,^{start(1,(NSString*)liveEndpoint,@"audio-probe");});
   dispatch_after(dispatch_time(DISPATCH_TIME_NOW,10*NSEC_PER_SEC),q,^{fail(1,@"audio probe timed out");});
   dispatch_main();
  }
  NSMutableData*line=[NSMutableData data];
  unsigned char bytes[4096];
  int result=0;
  while(YES) {
   @autoreleasepool {
    // NSFileHandle readDataOfLength: waits for the requested length or EOF.
    // A live pipe must dispatch short commands immediately, without closing stdin.
    ssize_t count=read(STDIN_FILENO,bytes,sizeof(bytes));
    if(count<0) {if(errno==EINTR)continue;result=2;break;}
    if(count==0)break;
    for(ssize_t i=0;i<count;i++) {
     if(bytes[i]==10) {
      NSDictionary*c=[NSJSONSerialization JSONObjectWithData:line options:0 error:NULL];
      if([c isKindOfClass:NSDictionary.class])dispatch_sync(q,^{handle(c);});
      [line setLength:0];
     } else {
      [line appendBytes:bytes+i length:1];
      if(line.length>131072) {result=2;break;}
     }
    }
    if(result)break;
   }
  }
  dispatch_sync(q,^{stop();});
  dispatch_sync(out,^{});
  return result;
 }
}
