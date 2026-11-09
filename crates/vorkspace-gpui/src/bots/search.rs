use super::*;

impl Bots {
    pub(crate) fn search_conversations(&self) -> Vec<Entity<Conversation>> {
        self.chats
            .iter()
            .chain(self.running.values().map(|run| (&run.bot, &run.view)))
            .filter(|(id, _)| !self.vorkspace_hidden.contains(*id))
            .map(|(_, view)| view.clone())
            .collect()
    }
    pub(crate) fn open_search_session(
        &mut self,
        session: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        let id = self.roster.of_session(session)?.id.clone();
        self.open(&id, window, cx);
        self.chats.get(&id).cloned()
    }
}
