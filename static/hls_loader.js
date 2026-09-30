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
