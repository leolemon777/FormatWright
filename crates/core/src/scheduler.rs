use std::collections::{BTreeMap, BTreeSet};

use formatwright_engine_sdk::Operation;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::Plan;

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkClass {
    Lightweight,
    IoHeavy,
    CpuHeavy,
    MemoryHeavy,
    Gpu,
    SerialEngine,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionBlocker {
    AlreadyActive,
    ProcessLimit,
    MemoryBudget,
    ExclusiveEngine,
    ClassLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRequest {
    pub job_id: Uuid,
    pub class: WorkClass,
    pub memory_bytes: u64,
    pub exclusivity_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerPolicy {
    pub max_processes: usize,
    pub max_cpu_heavy: usize,
    pub max_io_heavy: usize,
    pub max_gpu: usize,
    pub memory_budget_bytes: u64,
}

impl SchedulerPolicy {
    #[must_use]
    pub fn bounded(requested_processes: usize) -> Self {
        let max_processes = requested_processes.clamp(1, 16);
        let logical_cpus = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            max_processes,
            max_cpu_heavy: logical_cpus.div_ceil(2).clamp(1, max_processes),
            max_io_heavy: 2.min(max_processes),
            max_gpu: 1,
            memory_budget_bytes: 2 * 1024 * MIB,
        }
    }
}

#[derive(Debug)]
pub struct ResourceScheduler {
    policy: SchedulerPolicy,
    active: BTreeMap<Uuid, ResourceRequest>,
    exclusivity_keys: BTreeSet<String>,
    reserved_memory_bytes: u64,
}

impl ResourceScheduler {
    #[must_use]
    pub fn new(policy: SchedulerPolicy) -> Self {
        Self {
            policy,
            active: BTreeMap::new(),
            exclusivity_keys: BTreeSet::new(),
            reserved_memory_bytes: 0,
        }
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    #[must_use]
    pub fn reserved_memory_bytes(&self) -> u64 {
        self.reserved_memory_bytes
    }

    #[must_use]
    pub fn admission_blocker(&self, request: &ResourceRequest) -> Option<AdmissionBlocker> {
        if self.active.contains_key(&request.job_id) {
            return Some(AdmissionBlocker::AlreadyActive);
        }
        if self.active.len() >= self.policy.max_processes {
            return Some(AdmissionBlocker::ProcessLimit);
        }
        if self
            .reserved_memory_bytes
            .saturating_add(request.memory_bytes)
            > self.policy.memory_budget_bytes
        {
            return Some(AdmissionBlocker::MemoryBudget);
        }
        if request
            .exclusivity_key
            .as_ref()
            .is_some_and(|key| self.exclusivity_keys.contains(key))
        {
            return Some(AdmissionBlocker::ExclusiveEngine);
        }
        let same_class = self
            .active
            .values()
            .filter(|active| active.class == request.class)
            .count();
        let class_limit = match request.class {
            WorkClass::CpuHeavy => self.policy.max_cpu_heavy,
            WorkClass::IoHeavy => self.policy.max_io_heavy,
            WorkClass::Gpu => self.policy.max_gpu,
            WorkClass::Lightweight | WorkClass::MemoryHeavy | WorkClass::SerialEngine => {
                self.policy.max_processes
            }
        };
        (same_class >= class_limit).then_some(AdmissionBlocker::ClassLimit)
    }

    pub fn try_admit(&mut self, request: ResourceRequest) -> bool {
        if self.admission_blocker(&request).is_some() {
            return false;
        }
        self.reserved_memory_bytes = self
            .reserved_memory_bytes
            .saturating_add(request.memory_bytes);
        if let Some(key) = &request.exclusivity_key {
            self.exclusivity_keys.insert(key.clone());
        }
        self.active.insert(request.job_id, request);
        true
    }

    pub fn release(&mut self, job_id: Uuid) -> bool {
        let Some(request) = self.active.remove(&job_id) else {
            return false;
        };
        self.reserved_memory_bytes = self
            .reserved_memory_bytes
            .saturating_sub(request.memory_bytes);
        if let Some(key) = request.exclusivity_key {
            self.exclusivity_keys.remove(&key);
        }
        true
    }
}

#[must_use]
pub fn request_for_plan(job_id: Uuid, plan: &Plan) -> ResourceRequest {
    let mut class = WorkClass::Lightweight;
    let mut exclusivity_key = None;
    for step in &plan.steps {
        if step.engine.engine_id == "soffice" {
            class = WorkClass::SerialEngine;
            exclusivity_key = Some("soffice".to_owned());
            break;
        }
        if step.engine.engine_id == "msedge" {
            // One isolated browser instance prints at a time, mirroring the
            // office renderer's serialized profile semantics.
            class = WorkClass::SerialEngine;
            exclusivity_key = Some("msedge".to_owned());
            break;
        }
        if step.arguments.values().any(|value| {
            let value = value.to_ascii_lowercase();
            ["nvenc", "qsv", "vaapi", "videotoolbox"]
                .iter()
                .any(|marker| value.contains(marker))
        }) {
            class = WorkClass::Gpu;
            exclusivity_key = Some("gpu-encoder".to_owned());
            continue;
        }
        class = class.max(match step.operation {
            Operation::Inspect | Operation::Serialize => WorkClass::Lightweight,
            Operation::Remux | Operation::MetadataClean => WorkClass::IoHeavy,
            Operation::Transcode => WorkClass::CpuHeavy,
            Operation::Transform | Operation::Render => WorkClass::MemoryHeavy,
        });
    }
    let memory_bytes = match class {
        WorkClass::Lightweight => 64 * MIB,
        WorkClass::IoHeavy => 192 * MIB,
        WorkClass::CpuHeavy | WorkClass::SerialEngine => 1024 * MIB,
        WorkClass::Gpu => 512 * MIB,
        WorkClass::MemoryHeavy => 768 * MIB,
    };
    ResourceRequest {
        job_id,
        class,
        memory_bytes,
        exclusivity_key,
    }
}

impl Ord for WorkClass {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        rank(*self).cmp(&rank(*other))
    }
}

impl PartialOrd for WorkClass {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

const fn rank(class: WorkClass) -> u8 {
    match class {
        WorkClass::Lightweight => 0,
        WorkClass::IoHeavy => 1,
        WorkClass::CpuHeavy => 2,
        WorkClass::MemoryHeavy => 3,
        WorkClass::Gpu => 4,
        WorkClass::SerialEngine => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChangeSet, NetworkPolicy, PlanStep};
    use formatwright_engine_sdk::{Certification, EngineIdentity, LossClass};
    use std::path::PathBuf;

    fn request(job_id: Uuid, class: WorkClass, memory_bytes: u64) -> ResourceRequest {
        ResourceRequest {
            job_id,
            class,
            memory_bytes,
            exclusivity_key: None,
        }
    }

    #[test]
    fn enforces_process_memory_and_release_accounting() {
        let policy = SchedulerPolicy {
            max_processes: 2,
            max_cpu_heavy: 2,
            max_io_heavy: 2,
            max_gpu: 1,
            memory_budget_bytes: 100,
        };
        let mut scheduler = ResourceScheduler::new(policy);
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        assert!(scheduler.try_admit(request(first, WorkClass::Lightweight, 60)));
        assert!(!scheduler.try_admit(request(second, WorkClass::Lightweight, 50)));
        assert_eq!(scheduler.reserved_memory_bytes(), 60);
        assert!(scheduler.release(first));
        assert!(scheduler.try_admit(request(second, WorkClass::Lightweight, 50)));
    }

    #[test]
    fn serial_engine_key_prevents_overlap() {
        let mut scheduler = ResourceScheduler::new(SchedulerPolicy::bounded(4));
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let serial = |job_id| ResourceRequest {
            job_id,
            class: WorkClass::SerialEngine,
            memory_bytes: 256 * MIB,
            exclusivity_key: Some("soffice".to_owned()),
        };
        assert!(scheduler.try_admit(serial(first)));
        assert!(!scheduler.try_admit(serial(second)));
        assert!(scheduler.release(first));
        assert!(scheduler.try_admit(serial(second)));
    }

    #[test]
    fn default_memory_budget_limits_cpu_heavy_work_to_two() {
        let mut scheduler = ResourceScheduler::new(SchedulerPolicy::bounded(16));
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let third = Uuid::new_v4();
        assert!(scheduler.try_admit(request(first, WorkClass::CpuHeavy, 1024 * MIB)));
        assert!(scheduler.try_admit(request(second, WorkClass::CpuHeavy, 1024 * MIB)));
        let third_request = request(third, WorkClass::CpuHeavy, 1024 * MIB);
        assert_eq!(
            scheduler.admission_blocker(&third_request),
            Some(AdmissionBlocker::MemoryBudget)
        );
        assert!(!scheduler.try_admit(third_request));
        assert_eq!(scheduler.reserved_memory_bytes(), 2 * 1024 * MIB);
        assert!(scheduler.release(first));
        assert!(scheduler.try_admit(request(third, WorkClass::CpuHeavy, 1024 * MIB)));
    }

    #[test]
    fn reports_the_exact_scheduler_admission_blocker() {
        let policy = SchedulerPolicy {
            max_processes: 3,
            max_cpu_heavy: 1,
            max_io_heavy: 2,
            max_gpu: 1,
            memory_budget_bytes: 4 * 1024 * MIB,
        };
        let mut scheduler = ResourceScheduler::new(policy);
        let first = Uuid::new_v4();
        assert!(scheduler.try_admit(request(first, WorkClass::CpuHeavy, MIB)));

        let cpu_waiter = request(Uuid::new_v4(), WorkClass::CpuHeavy, MIB);
        assert_eq!(
            scheduler.admission_blocker(&cpu_waiter),
            Some(AdmissionBlocker::ClassLimit)
        );

        let serial = |job_id| ResourceRequest {
            job_id,
            class: WorkClass::SerialEngine,
            memory_bytes: MIB,
            exclusivity_key: Some("soffice".to_owned()),
        };
        assert!(scheduler.try_admit(serial(Uuid::new_v4())));
        assert_eq!(
            scheduler.admission_blocker(&serial(Uuid::new_v4())),
            Some(AdmissionBlocker::ExclusiveEngine)
        );

        assert!(scheduler.try_admit(request(Uuid::new_v4(), WorkClass::Lightweight, MIB)));
        assert_eq!(
            scheduler.admission_blocker(&request(Uuid::new_v4(), WorkClass::Lightweight, MIB)),
            Some(AdmissionBlocker::ProcessLimit)
        );
    }

    #[test]
    fn classifies_transcode_and_office_plans_conservatively() {
        let make_plan = |engine_id: &str, operation| Plan {
            schema_version: 1,
            plan_id: Uuid::nil(),
            plan_hash: "hash".to_owned(),
            input_fingerprint: "fingerprint".to_owned(),
            target_format: "target".to_owned(),
            constraints: BTreeMap::new(),
            steps: vec![PlanStep {
                step_id: "step".to_owned(),
                capability_id: "capability".to_owned(),
                engine: EngineIdentity {
                    engine_id: engine_id.to_owned(),
                    version: "1".to_owned(),
                    binary_path: PathBuf::from(engine_id),
                    binary_sha256: "sha".to_owned(),
                    manifest_sha256: None,
                    build_configuration: None,
                    certification: Certification::Experimental,
                },
                operation,
                loss_class: LossClass::None,
                arguments: BTreeMap::new(),
                estimated_temporary_bytes: None,
            }],
            changes: ChangeSet::default(),
            validators: vec![],
            network_policy: NetworkPolicy::Deny,
            output_path: None,
            estimated_output_bytes: None,
        };
        assert_eq!(
            request_for_plan(Uuid::new_v4(), &make_plan("ffmpeg", Operation::Transcode)).class,
            WorkClass::CpuHeavy
        );
        let office = request_for_plan(Uuid::new_v4(), &make_plan("soffice", Operation::Render));
        assert_eq!(office.class, WorkClass::SerialEngine);
        assert_eq!(office.exclusivity_key.as_deref(), Some("soffice"));
    }
}
