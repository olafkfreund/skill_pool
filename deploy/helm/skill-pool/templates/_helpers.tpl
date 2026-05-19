{{/*
Expand the name of the chart.
*/}}
{{- define "skill-pool.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Create a default fully qualified app name. Truncated to 63 chars to fit DNS
naming requirements.
*/}}
{{- define "skill-pool.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "skill-pool.server.fullname" -}}
{{- printf "%s-server" (include "skill-pool.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "skill-pool.web.fullname" -}}
{{- printf "%s-web" (include "skill-pool.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "skill-pool.migrate.fullname" -}}
{{- printf "%s-migrate" (include "skill-pool.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "skill-pool.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Common labels (apply to every object).
*/}}
{{- define "skill-pool.labels" -}}
helm.sh/chart: {{ include "skill-pool.chart" . }}
{{ include "skill-pool.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: skill-pool
{{- end -}}

{{- define "skill-pool.selectorLabels" -}}
app.kubernetes.io/name: {{ include "skill-pool.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "skill-pool.server.selectorLabels" -}}
{{ include "skill-pool.selectorLabels" . }}
app.kubernetes.io/component: server
{{- end -}}

{{- define "skill-pool.web.selectorLabels" -}}
{{ include "skill-pool.selectorLabels" . }}
app.kubernetes.io/component: web
{{- end -}}

{{- define "skill-pool.server.labels" -}}
{{ include "skill-pool.labels" . }}
app.kubernetes.io/component: server
{{- end -}}

{{- define "skill-pool.web.labels" -}}
{{ include "skill-pool.labels" . }}
app.kubernetes.io/component: web
{{- end -}}

{{- define "skill-pool.migrate.labels" -}}
{{ include "skill-pool.labels" . }}
app.kubernetes.io/component: migrate
{{- end -}}

{{/*
ServiceAccount name.
*/}}
{{- define "skill-pool.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "skill-pool.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/*
Resolve image references (falls back to .Chart.AppVersion when tag empty).
*/}}
{{- define "skill-pool.server.image" -}}
{{- $tag := default .Chart.AppVersion .Values.image.server.tag -}}
{{- printf "%s:%s" .Values.image.server.repository $tag -}}
{{- end -}}

{{- define "skill-pool.web.image" -}}
{{- $tag := default .Chart.AppVersion .Values.image.web.tag -}}
{{- printf "%s:%s" .Values.image.web.repository $tag -}}
{{- end -}}

{{- define "skill-pool.migrate.image" -}}
{{- printf "%s:%s" .Values.migrate.image.repository .Values.migrate.image.tag -}}
{{- end -}}

{{/*
Internal API base URL for the web tier.
Falls back to the in-cluster Service when the user hasn't overridden it.
*/}}
{{- define "skill-pool.internalApiBase" -}}
{{- $svc := include "skill-pool.server.fullname" . -}}
{{- $port := .Values.server.service.port | int -}}
{{- if eq $port 80 -}}
{{- printf "http://%s" $svc -}}
{{- else -}}
{{- printf "http://%s:%d" $svc $port -}}
{{- end -}}
{{- end -}}

{{/*
Web env block with computed SKILL_POOL_API_BASE fallback.
Used by deployment-web.yaml. Always returns a YAML list.
*/}}
{{- define "skill-pool.web.env" -}}
{{- range $k, $v := .Values.web.env }}
{{- if and (eq $k "SKILL_POOL_API_BASE") (eq ($v | toString) "") }}
- name: SKILL_POOL_API_BASE
  value: {{ include "skill-pool.internalApiBase" $ | quote }}
{{- else }}
- name: {{ $k }}
  value: {{ $v | quote }}
{{- end }}
{{- end }}
{{- with .Values.web.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{/*
Server env block (static k=v + extraEnv passthrough).
*/}}
{{- define "skill-pool.server.env" -}}
{{- range $k, $v := .Values.server.env }}
- name: {{ $k }}
  value: {{ $v | quote }}
{{- end }}
{{- with .Values.server.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{/*
Render an envFrom list. Skips empty entries.
*/}}
{{- define "skill-pool.envFrom" -}}
{{- with . }}
{{- toYaml . }}
{{- end }}
{{- end -}}
