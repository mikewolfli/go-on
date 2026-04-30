<template>
  <el-card>
    <template #header>
      <div class="skills-header">
        <span>{{ t("views.SkillsView.title") }}</span>
        <el-button size="small" type="primary" @click="showCreateDialog = true">
          {{ t("views.SkillsView.createSkill") }}
        </el-button>
        <el-button size="small" @click="showImportDialog = true">
          {{ t("views.SkillsView.importSkill") }}
        </el-button>
        <el-button size="small" :loading="loading" @click="fetchSkills">
          {{ t("common.refresh") }}
        </el-button>
      </div>
    </template>

    <!-- Loading State -->
    <div v-if="loading" class="skills-loading">
      <el-skeleton :rows="4" animated />
    </div>

    <!-- Empty State -->
    <el-empty v-else-if="skills.length === 0" :description="t('views.SkillsView.empty')" />

    <!-- Skills Table -->
    <el-table v-else :data="skills" stripe style="width: 100%">
      <el-table-column :label="t('common.name')" prop="name" min-width="140" />
      <el-table-column :label="t('common.description')" prop="description" min-width="200" show-overflow-tooltip />
      <el-table-column :label="t('views.SkillsView.version')" prop="version" width="100" />
      <el-table-column :label="t('common.status')" width="100">
        <template #default="{ row }">
          <el-switch
            :model-value="row.enabled"
            :loading="togglingName === row.name"
            @change="(val: boolean) => toggleSkill(row.name, val)"
          />
        </template>
      </el-table-column>
      <el-table-column :label="t('views.SkillsView.importedAt')" width="160">
        <template #default="{ row }">
          {{ formatDate(row.imported_at) }}
        </template>
      </el-table-column>
      <el-table-column :label="t('common.action')" width="180" fixed="right">
        <template #default="{ row }">
          <el-button
            size="small"
            :type="row.enabled ? 'warning' : 'success'"
            :loading="togglingName === row.name"
            @click="toggleSkill(row.name, !row.enabled)"
          >
            {{ row.enabled ? t('views.SkillsView.disable') : t('views.SkillsView.enable') }}
          </el-button>
          <el-button
            size="small"
            type="danger"
            :loading="removingName === row.name"
            @click="removeSkill(row.name)"
          >
            {{ t('views.SkillsView.remove') }}
          </el-button>
        </template>
      </el-table-column>
    </el-table>

    <!-- Create Skill Dialog -->
    <el-dialog v-model="showCreateDialog" :title="t('views.SkillsView.createSkill')" width="600px">
      <el-form :model="createForm" label-width="140px">
        <el-form-item :label="t('common.name')" required>
          <el-input v-model="createForm.name" :placeholder="t('views.SkillsView.namePlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('common.description')" required>
          <el-input
            v-model="createForm.description"
            type="textarea"
            :rows="2"
            :placeholder="t('views.SkillsView.descPlaceholder')"
          />
        </el-form-item>
        <el-form-item :label="t('views.SkillsView.promptTemplate')" required>
          <el-input
            v-model="createForm.prompt_template"
            type="textarea"
            :rows="4"
            :placeholder="t('views.SkillsView.promptPlaceholder')"
          />
        </el-form-item>
        <el-form-item :label="t('views.SkillsView.inputSchema')">
          <el-input
            v-model="createForm.inputSchemaText"
            type="textarea"
            :rows="3"
            :placeholder="t('views.SkillsView.schemaPlaceholder')"
          />
          <el-text size="small" type="info">{{ t('views.SkillsView.schemaHint') }}</el-text>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="showCreateDialog = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="creating" @click="handleCreate">{{ t('common.confirm') }}</el-button>
      </template>
    </el-dialog>

    <!-- Import Skill Dialog -->
    <el-dialog v-model="showImportDialog" :title="t('views.SkillsView.importSkill')" width="500px">
      <el-form label-width="100px">
        <el-form-item :label="t('views.SkillsView.sourceUrl')" required>
          <el-input
            v-model="importUrl"
            :placeholder="t('views.SkillsView.urlPlaceholder')"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="showImportDialog = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="importing" @click="handleImport">{{ t('common.confirm') }}</el-button>
      </template>
    </el-dialog>
  </el-card>
</template>

<script setup lang="ts">
import { onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import {
  listImportedSkills,
  importSkill,
  enableImportedSkill,
  disableImportedSkill,
  removeImportedSkill,
  createSkill,
  type ImportedSkillRecord,
} from "../services/rpcService";

const { t } = useI18n();

const loading = ref(false);
const skills = ref<ImportedSkillRecord[]>([]);
const togglingName = ref("");
const removingName = ref("");

// Create dialog
const showCreateDialog = ref(false);
const creating = ref(false);
const createForm = ref({
  name: "",
  description: "",
  prompt_template: "",
  inputSchemaText: "",
});

// Import dialog
const showImportDialog = ref(false);
const importing = ref(false);
const importUrl = ref("");

onMounted(() => {
  fetchSkills();
});

function formatDate(ts: number | undefined): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString();
}

async function fetchSkills() {
  loading.value = true;
  try {
    const result = await listImportedSkills();
    if (result.ok && result.skills) {
      skills.value = result.skills;
    } else {
      skills.value = [];
    }
  } catch (error) {
    ElMessage.error(String(error));
    skills.value = [];
  } finally {
    loading.value = false;
  }
}

async function toggleSkill(name: string, enabled: boolean) {
  togglingName.value = name;
  try {
    let result;
    if (enabled) {
      result = await enableImportedSkill(name);
    } else {
      result = await disableImportedSkill(name);
    }
    if (result.ok) {
      ElMessage.success(t("views.SkillsView.toastToggleSuccess"));
      await fetchSkills();
    } else {
      ElMessage.error(t("views.SkillsView.toastToggleFailed"));
    }
  } catch (error) {
    ElMessage.error(String(error));
  } finally {
    togglingName.value = "";
  }
}

async function removeSkill(name: string) {
  removingName.value = name;
  try {
    const result = await removeImportedSkill(name);
    if (result.ok) {
      ElMessage.success(t("views.SkillsView.toastRemoveSuccess"));
      await fetchSkills();
    } else {
      ElMessage.error(t("views.SkillsView.toastRemoveFailed"));
    }
  } catch (error) {
    ElMessage.error(String(error));
  } finally {
    removingName.value = "";
  }
}

async function handleCreate() {
  const { name, description, prompt_template, inputSchemaText } = createForm.value;
  if (!name.trim() || !description.trim() || !prompt_template.trim()) {
    ElMessage.warning(t("views.SkillsView.toastFillRequired"));
    return;
  }

  let input_schema: Record<string, string> = {};
  if (inputSchemaText.trim()) {
    try {
      const parsed = JSON.parse(inputSchemaText.trim());
      if (typeof parsed === "object" && !Array.isArray(parsed)) {
        input_schema = parsed;
      } else {
        ElMessage.warning(t("views.SkillsView.toastSchemaInvalid"));
        return;
      }
    } catch {
      // Try key=value line format
      for (const line of inputSchemaText.trim().split("\n")) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        const eqIdx = trimmed.indexOf("=");
        if (eqIdx > 0) {
          input_schema[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim();
        } else {
          ElMessage.warning(t("views.SkillsView.toastSchemaInvalid"));
          return;
        }
      }
    }
  }

  creating.value = true;
  try {
    const result = await createSkill({ name: name.trim(), description: description.trim(), prompt_template: prompt_template.trim(), input_schema });
    if (result.ok) {
      ElMessage.success(t("views.SkillsView.toastCreateSuccess"));
      showCreateDialog.value = false;
      createForm.value = { name: "", description: "", prompt_template: "", inputSchemaText: "" };
      await fetchSkills();
    } else {
      ElMessage.error(t("views.SkillsView.toastCreateFailed"));
    }
  } catch (error) {
    ElMessage.error(String(error));
  } finally {
    creating.value = false;
  }
}

async function handleImport() {
  const url = importUrl.value.trim();
  if (!url) {
    ElMessage.warning(t("views.SkillsView.toastFillRequired"));
    return;
  }

  // Detect GitHub URL pattern
  const githubMatch = url.match(/^https?:\/\/github\.com\/([^/]+\/[^/]+?)(?:\/tree\/([^/]+)(?:\/(.+))?)?(?:\?|$)/);
  let source: { kind: "github"; repo: string; ref: string; path?: string } | { kind: "url"; url: string };

  if (githubMatch) {
    source = {
      kind: "github",
      repo: githubMatch[1].replace(/\.git$/, ""),
      ref: githubMatch[2] || "main",
      path: githubMatch[3] || undefined,
    };
  } else {
    source = { kind: "url", url };
  }

  importing.value = true;
  try {
    const result = await importSkill(source);
    if (result.ok) {
      ElMessage.success(t("views.SkillsView.toastImportSuccess"));
      showImportDialog.value = false;
      importUrl.value = "";
      await fetchSkills();
    } else {
      ElMessage.error(t("views.SkillsView.toastImportFailed"));
    }
  } catch (error) {
    ElMessage.error(String(error));
  } finally {
    importing.value = false;
  }
}
</script>

<style scoped>
.skills-header {
  display: flex;
  align-items: center;
  gap: 8px;
}

.skills-loading {
  padding: 24px;
}
</style>
