<?php

declare(strict_types=1);

namespace App\Service;

use App\Model\User;
use InvalidArgumentException as BadInput;
use function strlen;
use const PHP_EOL;

#[Attribute(Attribute::TARGET_CLASS)]
final class Greeter extends BaseGreeter implements Stringable
{
    public const DEFAULT_NAME = 'monde';
    private static int $count = 0;

    public function __construct(
        private readonly string $greeting = "Bonjour",
        protected ?User $user = null,
    ) {
    }

    /** Says hello, éàü 😀 included. */
    public function greet(string $name = self::DEFAULT_NAME): string
    {
        // counter
        self::$count++;
        if ($name === '' || strlen($name) > 64) {
            throw new BadInput("Nom invalide : {$name}");
        }
        $result = match (true) {
            $this->user !== null => "Salut {$this->user->name}, $name",
            default => sprintf('%s %s%s', $this->greeting, $name, PHP_EOL),
        };
        $numbers = array_map(fn(int $n): int => $n ** 2, [1, 2.5, 0x1F, 1_000]);
        $piped = $result |> trim(...);
        return $piped . <<<TXT
        Total: {$numbers[0]}
        TXT;
    }

    public static function create(): static
    {
        return new static();
    }
}

enum Suit: string
{
    case Hearts = 'H';
    case Spades = 'S';
}
?>
<p>Fin du fichier</p>
