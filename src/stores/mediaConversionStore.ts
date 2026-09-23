import { create } from "zustand";
import type { MediaConversionJob } from "../lib/types";

interface MediaConversionState {
  jobs: MediaConversionJob[];
  upsert: (job: MediaConversionJob) => void;
  remove: (jobId: string) => void;
  clearFinished: () => void;
}

export const useMediaConversionStore = create<MediaConversionState>((set) => ({
  jobs: [],
  upsert: (job) => set((state) => ({
    jobs: state.jobs.some((item) => item.id === job.id)
      ? state.jobs.map((item) => item.id === job.id ? job : item)
      : [...state.jobs, job],
  })),
  remove: (jobId) => set((state) => ({ jobs: state.jobs.filter((job) => job.id !== jobId) })),
  clearFinished: () => set((state) => ({ jobs: state.jobs.filter((job) => job.status === "queued" || job.status === "running") })),
}));
