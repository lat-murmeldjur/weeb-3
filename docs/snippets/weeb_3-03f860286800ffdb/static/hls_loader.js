export function hlsConfig(live, liveSyncDuration) {
  const policy = { default: {
    maxTimeToFirstByteMs: 240000, maxLoadTimeMs: 250000, timeoutRetry: null,
    errorRetry: { maxNumRetry: 2, retryDelayMs: 250, maxRetryDelayMs: 2000 }
  } };
  return {
    progressive: true, autoStartLoad: false,
    maxBufferLength: live ? 90 : 180, maxMaxBufferLength: live ? 120 : 600,
    maxBufferSize: 128 * 1024 * 1024, backBufferLength: 15, maxBufferHole: 0.5,
    ...(live && { liveSyncDuration }),
    manifestLoadPolicy: policy, playlistLoadPolicy: policy, fragLoadPolicy: policy
  };
}

export async function loadHls() {
  const module = await import("hls.js");
  const Hls = module.default ?? module.Hls;
  class BufferController extends Hls.DefaultConfig.bufferController {
    constructor(hls, tracker) {
      super(hls, tracker);
      hls.on(Hls.Events.LEVEL_LOADED, (_, { details, levelInfo }) => {
        const previous = levelInfo?.details ?? (details.live && hls.latestLevelDetails);
        const offset = hls.config.timelineOffset;
        if (offset > 0 && previous && previous !== details &&
            (previous.live || details.live) && previous.appliedTimelineOffset === offset) {
          // Merging inherits the origin.
          details.appliedTimelineOffset = offset;
        }
      });
    }
  }
  return class extends Hls {
    constructor(config) {
      super({ ...config, bufferController: BufferController });
    }
  };
}
