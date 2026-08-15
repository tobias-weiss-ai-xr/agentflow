# SCS K3s Cluster - Security Hardening Taskfleet Configuration

This directory contains Taskfleet configuration for implementing a complete security hardening plan for the **SCS K3s Cluster** at Philipps-Universität Marburg.

---

## 🎯 Overview

Taskfleet is used to orchstrate **20 security hardening tasks** across 4 priority levels:

| Priority | Tasks |色Emoji | Description |
|----------|-------|--------|-------------|
| **P1 - CRITICAL** | 4 | 🔴 | Immediate action required |
| **P2 - HIGH** | 6 | 🟡 | This week |
| **P3 - MEDIUM** | 5 | 🟢 | This month |
| **P4 - LOW** | 4 | 🔵 | Future |
| **Documentation** | 1 | 📚 | Comprehensive docs |

**Total: 20 tasks, ~4-6 weeks of work**

---

## 📁 Structure

```
config/
├── workers-scs.json          # Worker configuration for SCS security tasks
└── tasks-scs-security.json    # All 20 security hardening tasks

prompts-scs/
└── scs-security-hardening.md # Agent prompt template for security tasks

SECURITY-SCS.md               # This file
```

---

## 🚀 Quick Start

### 1. Navigate to Taskfleet

```bash
cd /home/weissto_local/git/taskfleet
```

### 2. Point to SCS Repository

```bash
export TF_REPO_DIR=/home/weissto_local/git/scs
```

### 3. Use SCS-Specific Configuration

```bash
# Copy SCS config (or create symlinks)
cp config/workers-scs.json config/workers.json
cp config/tasks-scs-security.json config/tasks.json
```

### 4. Dry Run (Recommended)

```bash
./orchestrator.sh --dry-run --config-url config/tasks-scs-security.json
```

This shows what tasks would be dispatched without making any changes.

### 5. Run Security Tasks

```bash
# Run all security tasks
./orchestrator.sh --config-url config/tasks-scs-security.json

# Run specific priority
./orchestrator.sh --config-url config/tasks-scs-security.json --task P1-GRF-001

# Check status
./orchestrator.sh --status --config-url config/tasks-scs-security.json
```

---

## 📋 Task Overview by Priority

### 🔴 Priority 1: CRITICAL (Do Immediately)

| Task ID | Title | Scope | Acceptance |
|---------|-------|-------|------------|
| `P1-GRF-001` | Change Grafana admin password | Grafana role, docs | Password file exists, >=40 chars |
| `P1-K3S-001` | Enable K3s RBAC | K3s role, playbook | RBAC tasks exist, PodSecurity enabled |
| `P1-POD-001` | Apply Pod Security Standards | Network policies | Namespaces labeled with baseline |
| `P1-AUD-001` | Enable K3s audit logging | K3s audit config | Audit policy template exists |

**Estimated Time:** 1-2 days
**Risk:** Low-Medium (test in maintenance window)

---

### 🟡 Priority 2: HIGH (Do This Week)

| Task ID | Title | Scope | Acceptance |
|---------|-------|-------|------------|
| `P2-GRF-002` | Enable LDAP auth for Grafana | Grafana templates | LDAP config found |
| `P2-RBAC-001` | ArgoCD ServiceAccount + RBAC | ArgoCD manifests | RBAC resources defined |
| `P2-RBAC-002` | ServiceAccounts for all namespaces | ServiceAccount files | ServiceAccount files exist |
| `P2-NET-001` | NetworkPolicies for system ns | Network policy files | NetworkPolicy exists |
| `P2-SSL-001` | TLS for Prometheus/Alertmanager | TLS configs | TLS config exists |
| `P2-CON-001` | Harden containerd config | Containerd config | Security options configured |
| `P2-SEA-001` | Dedicated S3 user for backups | Backup secret | S3 user secret exists |
| `P2-SSH-001` | Harden SSH on all nodes | SSH hardening role | SSH hardening configured |

**Estimated Time:** 3-5 days
**Risk:** Medium

---

### 🟢 Priority 3: MEDIUM (Do This Month)

| Task ID | Title | Scope | Acceptance |
|---------|-------|-------|------------|
| `P3-KER-001` | CIS kernel parameters | Kernel hardening | Kernel params configured |
| `P3-ARG-001` | ArgoCD security review | ArgoCD configs | SSO/RBAC configured |
| `P3-NIX-001` | Nix builder security review | Nix builder manifests | Security context exists |
| `P3-TLS-001` | CA and cert rotation | Certificate management | CA role exists |
| `P3-NOD-001` | OS hardening (fail2ban, AIDE) | OS hardening role | Security tools configured |

**Estimated Time:** 1-2 weeks
**Risk:** Medium

---

### 🔵 Priority 4: LOW (Future)

| Task ID | Title | Scope | Acceptance |
|---------|-------|-------|------------|
| `P4-ZTA-001` | Zero Trust planning | Service mesh configs | Implementation planned |
| `P4-RUN-001` | Runtime security (Falco) | Falco deployment | Runtime monitoring exists |
| `P4-SEC-001` | Secrets manager (Vault/SS) | Secrets manager | Sealed Secrets/Vault exists |
| `P4-MFA-001` | MFA for all admin interfaces | MFA docs | MFA configured |

**Estimated Time:** 2-4 weeks
**Risk:** Medium-High

---

### 📚 Documentation

| Task ID | Title | Scope | Acceptance |
|---------|-------|-------|------------|
| `DOC-SEC-001` | Security Hardening docs | Security docs | Documentation comprehensive |

**Estimated Time:** 1-2 days
**Risk:** None

---

## 🎯 Recommended Execution Order

```mermaid
graph TD
    subgraph P1["🔴 Priority 1: CRITICAL"]
        A1[P1-GRF-001: Grafana Password] --> A2[P1-K3S-001: RBAC]
        A2 --> A3[P1-AUD-001: Audit Logging]
        A2 --> A4[P1-POD-001: PodSecurity]
    end
    
    subgraph P2["🟡 Priority 2: HIGH"]
        B1[P2-GRF-002: LDAP] --> A1
        B2[P2-SSH-001: SSH Hardening]
        B3[P2-NET-001: NetworkPolicies]
        B4[P2-SSL-001: TLS]
        B5[P2-CON-001: Containerd]
        B6[P2-RBAC-001: ArgoCD RBAC] --> A2
        B7[P2-RBAC-002: ServiceAccounts] --> B6
        B8[P2-SEA-001: S3 User] --> P2
    end
    
    subgraph P3["🟢 Priority 3: MEDIUM"]
        C1[P3-KER-001: Kernel] --> B2
        C2[P3-NOD-001: OS Hardening]
        C3[P3-TLS-001: CA] --> B4
        C4[P3-ARG-001: ArgoCD Security] --> B6
        C5[P3-NIX-001: Nix Builder] --> A4
    end
    
    subgraph P4["🔵 Priority 4: LOW"]
        D1[P4-RUN-001: Falco] --> C2
        D2[P4-SEC-001: Vault] --> C3
        D3[P4-MFA-001: MFA] --> B1
        D3 --> C4
        D4[P4-ZTA-001: Zero Trust] --> B3
        D4 --> D1
    end
    
    DOC[DOC-SEC-001: Documentation] --> A1
```

---

## 📊 Progress Tracking

### Current Status: Not Started

Run `./orchestrator.sh --status` to check progress.

### Milestones

- **Milestone 1 (Day 1):** P1 tasks complete (Critical)
- **Milestone 2 (Day 5):** P1 + P2 tasks complete (High)
- **Milestone 3 (Day 15):** P1 + P2 + P3 tasks complete (Medium)
- **Milestone 4 (Day 30):** All tasks complete (including Future)

---

## 🎯 Quick Commands Reference

```bash
# Set environment
export TF_REPO_DIR=/home/weissto_local/git/scs

# Switch to SCS security config
cp config/workers-scs.json config/workers.json
cp config/tasks-scs-security.json config/tasks.json

# Show status
./orchestrator.sh --status

# Dry run (see what would happen)
./orchestrator.sh --dry-run

# Run all tasks
./orchestrator.sh

# Run single task
./orchestrator.sh --task P1-GRF-001

# Run by worker
./orchestrator.sh --worker scs-security-01

# Run once (single dispatch cycle)
./orchestrator.sh --once

# Check what failed
./orchestrator.sh --status | grep -E "FAILED|failed"

# Continue from where you left off
./orchestrator.sh
```

---

## 💡 Tips and Best Practices

### 1. Start with Critical Tasks
```bash
# Run P1 tasks first
./orchestrator.sh --task P1-GRF-001  # Grafana password
./orchestrator.sh --task P1-K3S-001  # K3s RBAC
./orchestrator.sh --task P1-POD-001  # PodSecurity
./orchestrator.sh --task P1-AUD-001  # Audit logging
```

### 2. Test Individual Tasks
```bash
# Test Grafana password change
./orchestrator.sh --task P1-GRF-001 --dry-run

# If it looks good, run it
./orchestrator.sh --task P1-GRF-001

# Verify
cat /home/weissto_local/.grafana_password | wc -c
```

### 3. Run in Maintenance Window
```bash
# For tasks that affect cluster stability:
./orchestrator.sh --task P1-K3S-001  # During maintenance
./orchestrator.sh --task P1-POD-001  # May affect existing pods
```

### 4. Monitor Progress
```bash
# Watch status in real-time
watch -n 15 './orchestrator.sh --status'

# Or check periodically
./orchestrator.sh --status
```

### 5. Handle Failures
```bash
# Check failed tasks
./orchestrator.sh --status | grep FAILED

# Retry specific task
./orchestrator.sh --task <FAILED_TASK_ID>

# Increase attempts for difficult tasks
# Edit config/tasks-scs-security.json and increase max_attempts
```

---

## 📝 Task Details

Each task in `tasks-scs-security.json` has:

- **id**: Unique identifier (e.g., `P1-GRF-001`)
- **title**: Human-readable description
- **priority**: CRITICAL, HIGH, MEDIUM, LOW
- **deps**: Dependencies (tasks that must complete first)
- **scope**: Files and directories the task can modify
- **accept**: Acceptance command (must exit 0 to pass)
- **acceptance_prose**: Natural language success criteria
- **manual**: Whether manual intervention is required
- **tiers**: Which worker tiers can execute this task

---

## 🔒 Security Considerations

### Before Running Tasks

1. **Backup everything**
   ```bash
   cd /home/weissto_local/git/scs
   git commit -am "Pre-security-hardening backup"
   git push origin main
   ```

2. **Maintenance window** for critical tasks (P1-K3S-001, P1-POD-001)

3. **Test in staging** if possible

### After Running Tasks

1. **Verify cluster health**
   ```bash
   kubectl get nodes -o wide
   kubectl get pods -A
   kubectl top nodes
   ```

2. **Test critical applications**
   - ArgoCD sync
   - Grafana access
   - OpenDesk functionality
   - Backup jobs

3. **Document changes** in `docs/SECURITY-HARDENING.md`

---

## 📄 Generated Files

When tasks complete, they will create/modify files in the SCS repository:

```
/home/weissto_local/git/scs/
├── ansible/
│   ├── roles/
│   │   ├── grafana/           # Grafana security hardening
│   │   ├── k3s/               # K3s RBAC and audit logging
│   │   ├── ssh_hardening/    # SSH configuration
│   │   ├── containerd/       # Containerd security
│   │   ├── kernel_hardening/ # Kernel parameters
│   │   ├── os_hardening/     # OS-level security
│   │   └── certificates/     # Certificate management
│   └── playbooks/system/
│       ├── k3s-rbac.yml      # RBAC deployment
│       ├── podsecurity.yml   # PodSecurity Standards
│       ├── serviceaccounts.yml # ServiceAccount creation
│       └── mfa.yml           # MFA configuration
│
├── k8s/
│   ├── argocd/
│   │   └── rbac.yaml          # ArgoCD RBAC
│   ├── backup/
│   │   └── seaweedfs-backup-user-secret.yaml # S3 user for backups
│   ├── monitoring/
│   │   ├── prometheus-tls.yaml   # Prometheus TLS
│   │   └── alertmanager-tls.yaml # Alertmanager TLS
│   ├── networkpolicies/
│   │   ├── kube-system.yaml    # System namespace policies
│   │   └── kube-public.yaml    # Public namespace policies
│   ├── nix-builder/
│   │   └── securitycontext.yaml # Nix builder security
│   └── security/
│       ├── falco.yaml         # Runtime security
│       ├── sealed-secrets.yaml # Secrets manager
│       └── vault.yaml         # Alternative secrets manager
│
└── docs/
    ├── security/
    │   ├── SECURITY-HARDENING.md      # Main hardening doc
    │   ├── SECURITY-CHECKLIST.md      # Checklist
    │   ├── GRAFANA-SECURITY.md        # Grafana hardening
    │   ├── K3s-RBAC.md                # RBAC configuration
    │   ├── PODSECURITY.md              # Pod Security Standards
    │   ├── AUDIT-LOGGING.md           # Audit logging
    │   ├── SSH-HARDENING.md           # SSH configuration
    │   ├── CONTAINERD-SECURITY.md      # Containerd hardening
    │   ├── KERNEL-HARDENING.md        # Kernel parameters
    │   ├── ARGOCD-SECURITY.md          # ArgoCD security
    │   ├── NIX-BUILDER-SECURITY.md     # Nix builder security
    │   ├── SEAWEEDFS-SECURITY.md       # SeaweedFS security
    │   ├── CERTIFICATE-MANAGEMENT.md   # TLS certificates
    │   ├── NODE-HARDENING.md           # Node hardening
    │   ├── ZERO-TRUST-PLAN.md          # Zero Trust planning
    │   ├── RUNTIME-SECURITY.md         # Runtime security
    │   ├── SECRETS-MANAGEMENT.md       # Secrets management
    │   └── MFA-IMPLEMENTATION.md       # MFA configuration
    └── SECURITY-HARDENING-PLAN.md      # Complete plan
```

---

## 🎉 Completion Checklist

- [ ] All P1 (Critical) tasks complete
- [ ] All P2 (High) tasks complete
- [ ] All P3 (Medium) tasks complete
- [ ] All P4 (Low) tasks complete
- [ ] Comprehensive documentation created
- [ ] Cluster health verified after each phase
- [ ] All acceptance gates passing
- [ ] Changes committed and pushed to Git

---

## 📞 Support

### Taskfleet Support
- **Repository:** `https://github.com/tobias-weiss-ai-xr/taskfleet`
- **Documentation:** See `README.md` and `ROADMAP.md` in taskfleet repo
- **Testing:** Run `bash tests/run-all-tests.sh` to verify taskfleet installation

### SCS Security Support
- **Questions:** scs-team@hrz.uni-marburg.de
- **Slack:** #scs-kubernetes
- **Emergency:** HRZ System Administration

---

## 📅 Version History

| Date | Version | Description | Author |
|------|---------|-------------|--------|
| 15. Aug 2026 | 1.0 | Initial Taskfleet configuration for SCS security hardening | AI Assistant + SCS Team |

---

*© 2026 HRZ - Hochschulrechenzentrum Philipps-Universität Marburg*
