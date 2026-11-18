<?php

declare(strict_types=1);

namespace Demo\Notes;

use InvalidArgumentException;

enum Level: string
{
    case Low = 'low';
    case High = 'high';
}

#[Attribute(Attribute::TARGET_CLASS)]
final class Note
{
    public const MAX_TITLE = 40;

    public function __construct(
        public readonly int $id,
        public readonly string $title,
        private array $tags = [],
        public Level $level = Level::Low,
    ) {
        if ($title === '') {
            throw new InvalidArgumentException("Empty title");
        }
    }

    /** Label shown in the list. */
    public function label(): string
    {
        // match replaces a switch
        return match ($this->level) {
            Level::High => "★ {$this->title}",
            default => sprintf('%s (%d)', mb_substr($this->title, 0, self::MAX_TITLE), count($this->tags)),
        };
    }
}

$notes = array_map(
    fn(array $row): Note => new Note(...$row),
    [[1, 'Courses', ['vrac'], Level::High], [2, 'Idées']],
);

foreach ($notes as $note) {
    echo $note->label(), PHP_EOL;
}
