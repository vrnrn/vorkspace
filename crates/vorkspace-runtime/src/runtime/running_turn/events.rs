use super::*;
impl RunningTurn {
    pub(super) fn events(
        &mut self,
        events: Vec<Event>,
        kind: Kind,
        activity_cwd: &str,
        visual_bridge: Option<&Server>,
    ) -> Result<(), String> {
        // Cancellation freezes the transcript, even while the native
        // provider drains its interrupt acknowledgement. A provider
        // completion already received in this batch must still be
        // durably recorded if Stop races with its final publication.
        let completion_already_received = events.iter().any(|event| matches!(event, Event::Done));
        if self.cancel.load(Ordering::Relaxed) && !completion_already_received {
            return Err("Stopped".into());
        }
        let publications = visual_bridge
            .map(|bridge| bridge.drain())
            .unwrap_or_default();
        if events.is_empty() && publications.is_empty() {
            return Ok(());
        }
        self.from = self.transcript.rows.len();
        self.update = Update {
            status: "Streaming…".to_string(),
            ..Default::default()
        };
        self.context_changed = false;
        self.completed_batch = false;
        self.boundary = false;
        for event in events {
            if matches!(&event, Event::Ready | Event::Done)
                && let Some(prepared) = self.handoff.as_ref().filter(|_| !self.handoff_accepted)
            {
                self.handoff_accepted = true;
                self.handoff_retired = locked(&self.store)?.settle_local_provider_handoff(
                    self.session,
                    &self.input.summary.provider,
                    &self.account,
                    prepared,
                    None,
                )?;
                if self.handoff_retired {
                    self.update.handoff = Some(None);
                }
            }
            match event {
                Event::Commands(discovered) => self.commands(discovered),
                Event::TurnId(id) => self.turn_id(id),
                Event::Ready => self.ready(),
                Event::FollowUpDispatch(message) => self.follow_up_dispatch(message)?,
                Event::FollowUp(message) => self.follow_up(message, kind)?,
                Event::FollowUpFailed { id, error } => self.follow_up_failed(id, error)?,
                Event::BeginAssistant => self.begin_assistant(),
                Event::Text(text) => self.text(text),
                Event::Reasoning(text) => self.reasoning(text),
                Event::Tool(tool) => self.tool(tool, activity_cwd)?,
                Event::SessionId(id) => self.session_id(id)?,
                Event::Model(id) => self.model(id),
                Event::ConfigChanged {
                    model: changed_model,
                    model_settings,
                } => self.config_changed(changed_model, model_settings)?,
                Event::Interjection {
                    text,
                    custom_type,
                    severity,
                } => self.interjection(text, custom_type, severity)?,
                Event::Metrics(reading) => self.metrics(reading),
                Event::Context { used, window } => self.context(used, window),
                Event::Approval(approval) => self.approval(approval),
                Event::ApprovalResolved(id) => self.approval_resolved(id),
                Event::Question(question) => self.question(question),
                Event::QuestionResolved(id) => self.question_resolved(id),
                Event::Status(text) => self.status(text),
                Event::UsageLimited(limit) => self.usage_limited(limit),
                Event::Done => self.done(),
            }
        }
        self.publish_events(publications)
    }
    fn commands(&mut self, discovered: Vec<provider::NativeCommand>) {
        self.update.commands = Some(discovered);
    }
    fn turn_id(&mut self, id: String) {
        if let Some((_, info)) = &mut self.turn_info {
            info.provider_turn_id = Some(id.clone());
        }
        self.update.provider_turn_id = Some(id);
    }
    fn ready(&mut self) {
        self.update.turn_ready = true;
        self.boundary = true;
    }
    fn follow_up_dispatch(&mut self, message: Message) -> Result<(), String> {
        self.submissions.dispatch_follow_up(&message)?;
        Ok(())
    }
    fn follow_up(&mut self, message: Message, kind: Kind) -> Result<(), String> {
        let at = self.transcript.begin("user");
        self.activity.prompt(at);
        self.from = self.from.min(at);
        self.transcript.append(&message.text);
        self.transcript.attach_files(&message.attachments);
        // Paint the working receipt with the accepted steer,
        // even when the provider has no new output yet.
        let turn = self.transcript.begin("assistant");
        self.pending.accept(
            &self.store,
            self.session,
            at,
            &self.transcript,
            &message.id,
            std::time::Instant::now(),
        )?;
        // The durable queue/transcript handoff owns acceptance.
        // Keep its ID even if a later metadata write fails.
        self.accepted_ids.push(message.id.clone());
        self.submissions.accept_follow_up(&message.id)?;
        if let Some((_, root)) = &self.turn_info {
            // A steer keeps its send time and model identity,
            // while timing/metrics belong to the original run.
            let steer = TurnInfo {
                provider: root.provider.clone(),
                model: root.model.clone(),
                model_id: root.model_id.clone(),
                provider_turn_id: if kind == Kind::Claude && self.bot.is_some() {
                    Some(provider_history::input_uuid(&message.id))
                } else {
                    root.provider_turn_id.clone()
                },
                ..Default::default()
            };
            self.pending
                .receipt(&self.store, self.session, turn, steer)?;
        }
        self.update.follow_up_delivered.push(message.id);
        Ok(())
    }
    fn follow_up_failed(&mut self, id: String, error: String) -> Result<(), String> {
        self.submissions.fail_follow_up(&id, &error)?;
        self.update.follow_up_errors.push((id, error));
        Ok(())
    }
    fn begin_assistant(&mut self) {
        if self
            .transcript
            .rows
            .last()
            .is_none_or(|r| r.role != "assistant" || !r.text.is_empty())
        {
            let at = self.transcript.begin("assistant");
            self.from = self.from.min(at);
            self.pending.changed(at);
        };
    }
    fn text(&mut self, text: String) {
        if self
            .transcript
            .rows
            .last()
            .is_none_or(|r| r.role != "assistant")
        {
            self.from = self.from.min(self.transcript.begin("assistant"));
        }
        let at = self.transcript.append(&text);
        self.from = self.from.min(at);
        self.pending.changed(at);
    }
    fn reasoning(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self
            .transcript
            .rows
            .last()
            .is_none_or(|r| r.role != "reasoning")
        {
            self.from = self.from.min(self.transcript.begin("reasoning"));
        }
        let at = self.transcript.append(&text);
        self.from = self.from.min(at);
        self.pending.changed(at);
    }
    fn tool(&mut self, tool: ToolCall, activity_cwd: &str) -> Result<(), String> {
        if tool.is_compaction() {
            self.update.status = if tool.state == ToolState::Running {
                "Compacting context…"
            } else {
                "Working…"
            }
            .into();
        }
        let ix = *self
            .tool_rows
            .entry(tool.id.clone())
            .or_insert_with(|| self.transcript.begin("tool"));
        self.activity.tool(ix, &tool, activity_cwd);
        self.transcript.rows[ix].text = serde_json::to_string(&tool).map_err(|e| e.to_string())?;
        self.from = self.from.min(ix);
        self.pending.changed(ix);
        Ok(())
    }
    fn session_id(&mut self, id: String) -> Result<(), String> {
        if self.provider_id.as_ref() != Some(&id) {
            let saved = locked(&self.store)?.record_provider_session(
                self.source(),
                &id,
                self.bot.is_some(),
            )?;
            if saved {
                self.binding = ProviderBinding::Set(Some(id.clone()));
                self.update.binding = self.binding.clone();
                if self.bot.is_some() {
                    self.context = vorkspace_core::context::Usage::default();
                    self.context_changed = true;
                    self.provider_reset = true;
                }
            }
            self.provider_id = Some(id.clone());
            self.pending.own_context_thread(Some(&id));
        }
        Ok(())
    }
    fn model(&mut self, id: String) {
        self.update.model = Some(id);
    }
    fn config_changed(
        &mut self,
        changed_model: Option<String>,
        model_settings: HashMap<String, String>,
    ) -> Result<(), String> {
        if changed_model.is_some() {
            self.update.model = changed_model.clone();
        }
        let patch = ConfigurationPatch {
            model: changed_model,
            model_settings,
        };
        let source = self.source();
        if locked(&self.store)?.record_provider_configuration(source, &patch)? {
            let saved = ProviderConfiguration {
                revision: self.revision,
                provider_id: self.provider_id.clone(),
                patch,
            };
            saved.clone().merge_into(&mut self.configuration);
            saved.merge_into(&mut self.update.configuration);
        }
        Ok(())
    }
    fn interjection(
        &mut self,
        text: String,
        custom_type: String,
        severity: Option<String>,
    ) -> Result<(), String> {
        let note = vorkspace_core::transcript::Interjection {
            text,
            custom_type,
            severity,
        };
        let at = self.transcript.begin("interjection");
        self.transcript
            .append(&serde_json::to_string(&note).map_err(|e| e.to_string())?);
        self.from = self.from.min(at);
        self.pending.changed(at);
        self.boundary = true;
        Ok(())
    }
    fn metrics(&mut self, reading: TurnMetrics) {
        if let Some((_, info)) = &mut self.turn_info {
            info.metrics = Some(reading);
        }
        self.update.metrics = Some(reading);
    }
    fn context(&mut self, used: Option<u64>, window: Option<u64>) {
        self.context.merge(used, window);
        self.context_changed = true;
        self.pending.context_changed();
    }
    fn approval(&mut self, approval: Approval) {
        self.update.status = "Waiting for approval".into();
        self.update.approvals.push(approval);
        self.boundary = true;
    }
    fn approval_resolved(&mut self, id: String) {
        self.update.approvals_resolved.push(id);
        self.boundary = true;
    }
    fn question(&mut self, question: Question) {
        self.update.status = "Waiting for your answer".into();
        self.update.questions.push(question);
        self.boundary = true;
    }
    fn question_resolved(&mut self, id: String) {
        self.update.questions_resolved.push(id);
        self.boundary = true;
    }
    fn status(&mut self, text: String) {
        self.update.status = text;
    }
    fn usage_limited(&mut self, limit: Limit) {
        self.usage_limit = Some(limit);
    }
    fn done(&mut self) {
        self.completed_batch = true;
        self.update.status = "Ready".into();
        self.boundary = true;
    }
    fn publish_events(&mut self, publications: Vec<Publication>) -> Result<(), String> {
        for publication in &publications {
            let document =
                serde_json::to_string(&publication.document).map_err(|e| e.to_string())?;
            let at = self.transcript.begin("visual");
            self.transcript.append(&document);
            self.from = self.from.min(at);
            self.pending.changed(at);
            self.boundary = true;
        }
        let now = Instant::now();
        let publication_result = (|| {
            if self.pending.due(self.boundary, now) {
                self.flush(now)?;
            }
            // The provider's terminal signal is authoritative once its
            // accompanying transcript changes are durable. A later UI
            // publication failure must not turn this send uncertain.
            if self.completed_batch {
                self.provider_completed = true;
            }
            let mut update = std::mem::take(&mut self.update);
            update.activity = self.activity.label();
            update.transcript =
                TranscriptPatch::tail(self.from, self.transcript.rows[self.from..].to_vec());
            update.context = self.context_changed.then_some(self.context);
            publish(&self.tx, &self.cancel, update)
        })();
        // A tool succeeds only after both durable storage and the live
        // transcript update have accepted the visual.
        for publication in publications {
            let result = publication_result.as_ref().map(|_| {
                serde_json::json!({"id":publication.document.id,"title":publication.document.title,"saved":true})
            }).map_err(Clone::clone);
            let _ = publication.reply.send(result);
        }
        publication_result
    }
}
