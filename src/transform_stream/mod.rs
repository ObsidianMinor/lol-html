mod dispatcher;

use self::dispatcher::Dispatcher;
use crate::base::{Buffer, Chunk};

use crate::parser::{Parser, ParserDirective, SharedAttributeBuffer};
use encoding_rs::Encoding;
use failure::{Error, ResultExt};
use std::cell::RefCell;
use std::rc::Rc;

pub use self::dispatcher::{
    AuxStartTagInfo, OutputSink, StartTagHandlingResult, TransformController,
};

const BUFFER_ERROR_CONTEXT: &str = concat!(
    "This is caused by the parser encountering an extremely long ",
    "tag or a comment that is captured by the specified selector."
);

pub struct TransformStream<C, O>
where
    C: TransformController,
    O: OutputSink,
{
    dispatcher: Rc<RefCell<Dispatcher<C, O>>>,
    parser: Parser<Dispatcher<C, O>>,
    buffer: Buffer,
    has_buffered_data: bool,
    finished: bool,
}

impl<C, O> TransformStream<C, O>
where
    C: TransformController,
    O: OutputSink,
{
    pub fn new(
        transform_controller: C,
        output_sink: O,
        buffer_capacity: usize,
        encoding: &'static Encoding,
    ) -> Self {
        let initial_parser_directive = if transform_controller.initial_capture_flags().is_empty() {
            ParserDirective::WherePossibleScanForTagsOnly
        } else {
            ParserDirective::Lex
        };

        let dispatcher = Rc::new(RefCell::new(Dispatcher::new(
            transform_controller,
            output_sink,
            encoding,
        )));

        TransformStream {
            parser: Parser::new(&dispatcher, initial_parser_directive),
            dispatcher,
            buffer: Buffer::new(buffer_capacity),
            has_buffered_data: false,
            finished: false,
        }
    }

    fn buffer_blocked_bytes(
        &mut self,
        data: &[u8],
        blocked_byte_count: usize,
    ) -> Result<(), Error> {
        if self.has_buffered_data {
            self.buffer.shrink_to_last(blocked_byte_count);
        } else {
            let blocked_bytes = &data[data.len() - blocked_byte_count..];

            self.buffer
                .init_with(blocked_bytes)
                .context(BUFFER_ERROR_CONTEXT)?;

            self.has_buffered_data = true;
        }

        trace!(@buffer self.buffer);

        Ok(())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        assert!(
            !self.finished,
            "Data was written into the stream after it has ended."
        );

        trace!(@write data);

        let chunk = if self.has_buffered_data {
            self.buffer.append(data).context(BUFFER_ERROR_CONTEXT)?;
            self.buffer.bytes()
        } else {
            data
        }
        .into();

        trace!(@chunk chunk);

        let blocked_byte_count = self.parser.parse(&chunk)?;

        self.dispatcher
            .borrow_mut()
            .flush_remaining_input(&chunk, blocked_byte_count);

        if blocked_byte_count > 0 {
            self.buffer_blocked_bytes(data, blocked_byte_count)?;
        } else {
            self.has_buffered_data = false;
        }

        Ok(())
    }

    pub fn end(&mut self) -> Result<(), Error> {
        assert!(!self.finished, "Stream was ended twice.");
        trace!(@end);

        self.finished = true;

        let chunk = if self.has_buffered_data {
            Chunk::last(self.buffer.bytes())
        } else {
            Chunk::last_empty()
        };

        trace!(@chunk chunk);

        self.parser.parse(&chunk)?;
        self.dispatcher.borrow_mut().finish(&chunk);

        Ok(())
    }

    #[cfg(feature = "test_api")]
    pub fn parser(&mut self) -> &mut Parser<Dispatcher<C, O>> {
        &mut self.parser
    }
}
